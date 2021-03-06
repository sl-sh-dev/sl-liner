use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{cmp, mem};

use sl_console::event::{Key, KeyCode, KeyMod};

use crate::buffer::Buffer;
use crate::Editor;
use crate::KeyMap;

pub trait ViKeywordRule {
    /// All alphanumeric characters and _ are considered valid for keywords in vi by default.
    fn is_vi_keyword(&self, str: &str) -> bool {
        let mut ret = false;
        if str == "_" {
            ret = true
        } else if !str.trim().is_empty() {
            for c in str.chars() {
                if c.is_alphanumeric() {
                    ret = true;
                } else {
                    ret = false;
                    break;
                }
            }
        }
        ret
    }
}

pub struct DefaultViKeywordRule;

impl ViKeywordRule for DefaultViKeywordRule {}

impl Default for DefaultViKeywordRule {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultViKeywordRule {
    pub fn new() -> Self {
        DefaultViKeywordRule {}
    }
}

pub struct AlphanumericAndVariableKeywordRule<'a> {
    treat_as_keyword: Vec<&'a str>,
}

impl ViKeywordRule for AlphanumericAndVariableKeywordRule<'_> {
    fn is_vi_keyword(&self, str: &str) -> bool {
        let mut ret = false;
        if self.treat_as_keyword.contains(&str) {
            ret = true
        } else if !str.trim().is_empty() {
            for c in str.chars() {
                if c.is_alphanumeric() {
                    ret = true;
                } else {
                    ret = false;
                    break;
                }
            }
        }
        ret
    }
}

impl<'a> AlphanumericAndVariableKeywordRule<'a> {
    pub fn new(treat_as_keyword: Vec<&'a str>) -> Self {
        AlphanumericAndVariableKeywordRule { treat_as_keyword }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharMovement {
    RightUntil,
    RightAt,
    LeftUntil,
    LeftAt,
    Repeat,
    ReverseRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveType {
    Inclusive,
    Exclusive,
}

/// defines the two modes available to text objects, a and w. TextObjectModes
/// are preceded by the commands c, d, and y, and followed by a
/// TextObjectMovement which specified the range of characters in the buffer to
/// which the command applies. a indicates the whole range including the
/// surrounding characters and i indicates the inner range excluding the
/// surrounding characters. The commands yi' and ya' on the string
/// (bars indicate cursor position):
/// "the 'wh|o|le' point"
///       ^____^ inner range = yi' results in paste buffer of whole
///      ^______^ outer range = ya' results in paste bufer of 'whole'
/// This is because a includes the surrounding characters in the
/// TextObjectMovement for apostrophe and i excludes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextObjectMode {
    Whole,
    Inner,
}

impl TextObjectMode {
    fn from_key_code(k: KeyCode) -> Option<TextObjectMode> {
        match k {
            KeyCode::Char('a') => Some(TextObjectMode::Whole),
            KeyCode::Char('i') => Some(TextObjectMode::Inner),
            _ => None,
        }
    }
}

/// TextObjectMovement defines the range of characters that will be applied to
/// the command chosen: c, d, or y.
/// Supported text object movements:
///     w   vi keyword (all alphanumeric characters and _) surrounded by whitespace
///     (   area surrounded by ( and )
///     )   area surrounded by ( and )
///     {   area surrounded by { and }
///     }   area surrounded by { and }
///     [   area surrounded by [ and ]
///     ]   area surrounded by [ and ]
///     `   area surrounded by ` characters
///     '   area surrounded by ' characters
///     "   area surrounded by " characters
///     t   area surrounded by > and <
///     >   area surrounded by < and >
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextObjectMovement {
    Word,
    Surround(char, char),
}

impl TextObjectMovement {
    fn from_key_code(k: KeyCode) -> Option<TextObjectMovement> {
        match k {
            KeyCode::Char('w') => Some(TextObjectMovement::Word),
            KeyCode::Char('(') => Some(TextObjectMovement::Surround('(', ')')),
            KeyCode::Char(')') => Some(TextObjectMovement::Surround('(', ')')),
            KeyCode::Char('{') => Some(TextObjectMovement::Surround('{', '}')),
            KeyCode::Char('}') => Some(TextObjectMovement::Surround('{', '}')),
            KeyCode::Char('[') => Some(TextObjectMovement::Surround('[', ']')),
            KeyCode::Char(']') => Some(TextObjectMovement::Surround('[', ']')),
            KeyCode::Char('`') => Some(TextObjectMovement::Surround('`', '`')),
            KeyCode::Char('\'') => Some(TextObjectMovement::Surround('\'', '\'')),
            KeyCode::Char('"') => Some(TextObjectMovement::Surround('"', '"')),
            KeyCode::Char('t') => Some(TextObjectMovement::Surround('>', '<')),
            KeyCode::Char('>') => Some(TextObjectMovement::Surround('<', '>')),
            _ => None,
        }
    }
}

/// The editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Insert,
    Normal,
    Replace,
    Delete(usize),
    Yank(usize),
    TextObject(TextObjectMode),
    MoveToChar(CharMovement),
    G,
    Tilde,
}

#[derive(Debug, Clone)]
struct ModeStack(Vec<Mode>);

impl ModeStack {
    fn with_insert() -> Self {
        ModeStack(vec![Mode::Insert])
    }

    /// Get the current mode.
    ///
    /// If the stack is empty, we are in normal mode.
    fn mode(&self) -> Mode {
        self.0.last().cloned().unwrap_or(Mode::Normal)
    }

    /// Empty the stack and return to normal mode.
    fn clear(&mut self) {
        self.0.clear()
    }

    /// Push the given mode on to the stack.
    fn push(&mut self, m: Mode) {
        self.0.push(m)
    }

    fn pop(&mut self) -> Mode {
        self.0.pop().unwrap_or(Mode::Normal)
    }
}

fn is_movement_key_to_right(key: Key) -> bool {
    matches!(
        key.code,
        KeyCode::Char('l')
            | KeyCode::Right
            | KeyCode::Char('w')
            | KeyCode::Char('W')
            | KeyCode::Char('e')
            | KeyCode::Char('E')
            | KeyCode::Char(' ')
            | KeyCode::End
            | KeyCode::Char('$')
            | KeyCode::Char('t')
            | KeyCode::Char('f')
    )
}

fn is_movement_key(key: Key) -> bool {
    matches!(
        key.code,
        KeyCode::Char('h')
            | KeyCode::Char('l')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('w')
            | KeyCode::Char('W')
            | KeyCode::Char('b')
            | KeyCode::Char('B')
            | KeyCode::Char('e')
            | KeyCode::Char('E')
            | KeyCode::Char('g')
            | KeyCode::Backspace
            | KeyCode::Char(' ')
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char('^')
            | KeyCode::Char('$')
            | KeyCode::Char('t')
            | KeyCode::Char('f')
            | KeyCode::Char('T')
            | KeyCode::Char('F')
            | KeyCode::Char(';')
            | KeyCode::Char(',')
    )
}

#[derive(PartialEq, Clone, Copy)]
enum ViMoveMode {
    Keyword,
    Whitespace,
}

#[derive(PartialEq, Clone, Copy)]
enum ViMoveDir {
    Left,
    Right,
}

impl ViMoveDir {
    pub fn advance(self, cursor: &mut usize, max: usize) -> bool {
        self.move_cursor(cursor, max, self)
    }

    pub fn go_back(self, cursor: &mut usize, max: usize) -> bool {
        match self {
            ViMoveDir::Right => self.move_cursor(cursor, max, ViMoveDir::Left),
            ViMoveDir::Left => self.move_cursor(cursor, max, ViMoveDir::Right),
        }
    }

    fn move_cursor(self, cursor: &mut usize, max: usize, dir: ViMoveDir) -> bool {
        if dir == ViMoveDir::Right && *cursor == max {
            return false;
        }

        if dir == ViMoveDir::Left && *cursor == 0 {
            return false;
        }

        match dir {
            ViMoveDir::Right => *cursor += 1,
            ViMoveDir::Left => *cursor -= 1,
        };
        true
    }
}

fn find_char(buf: &Buffer, start: usize, ch: char, count: usize) -> Option<usize> {
    assert!(count > 0);
    let mut offset = None;
    let str = &ch.to_string();
    let mut count = count;
    for (i, s) in buf.range_graphemes_all().enumerate().skip(start) {
        if s == str {
            if count == 1 {
                offset = Some(i);
                break;
            } else {
                count -= 1;
            }
        }
    }
    offset
}

fn find_char_rev(buf: &Buffer, start: usize, ch: char, count: usize) -> Option<usize> {
    assert!(count > 0);
    let rstart = buf.num_graphemes() - start;
    let mut offset = None;
    let str = &ch.to_string();
    let mut count = count;
    for (i, s) in buf.range_graphemes_all().enumerate().rev().skip(rstart) {
        if s == str {
            if count == 1 {
                offset = Some(i);
                break;
            } else {
                count -= 1;
            }
        }
    }
    offset
}

fn find_char_balance_delim(
    buf: &Buffer,
    start: usize,
    to_find: char,
    to_find_opposite: char,
    count: usize,
) -> Option<usize> {
    assert!(count > 0);
    let iter = buf.range_graphemes_from(start).enumerate();
    let to_skip = |i| start + i;
    find_balance_delim(to_find, to_find_opposite, count, to_skip, Box::new(iter))
}

fn find_char_rev_balance_delim(
    buf: &Buffer,
    start: usize,
    to_find: char,
    to_find_opposite: char,
    count: usize,
) -> Option<usize> {
    assert!(count > 0);
    let rstart = buf.num_graphemes() - start;
    let iter = buf.range_graphemes_until(start).rev().enumerate();
    let to_skip = |i| buf.num_graphemes() - rstart - i - 1;
    find_balance_delim(to_find, to_find_opposite, count, to_skip, Box::new(iter))
}

/// searches through string for matching character but refuses to match
/// characters if they are unbalanced, used for matching pairs of (), {}, and []
fn find_balance_delim<F>(
    to_find: char,
    to_find_opposite: char,
    count: usize,
    to_skip: F,
    iter: Box<dyn Iterator<Item = (usize, &str)> + '_>,
) -> Option<usize>
where
    F: Fn(usize) -> usize,
{
    let mut count = count;
    let mut balance = 0;
    for (i, (_, c)) in iter.enumerate() {
        // if the current character is equal to the opposite delim of the to_find
        // char, i.e. searching for a matching open paren and encountering a
        // close paren, then the close paren must be added to the stack and
        // popped only when another to_find char is found. An idx is returned
        // only when the to_find character is found and the stack is empty.
        if c[..] == to_find_opposite.to_string() {
            balance += 1;
        } else if c[..] == to_find.to_string() {
            if balance == 0 {
                if count == 1 {
                    return Some(to_skip(i));
                } else {
                    count -= 1;
                }
            } else {
                balance -= 1;
            }
        }
    }
    None
}

/// Vi keybindings for `Editor`.
///
/// ```
/// use sl_liner::*;
/// use sl_liner::keymap;
///
/// struct EmptyCompleter;
/// impl Completer for EmptyCompleter {
///     fn completions(&mut self, _start: &str) -> Vec<String> {
///         Vec::new()
///     }
/// }
///
/// let mut context = Context::new();
/// context.set_keymap(Box::new(keymap::Vi::new()));
/// // This will hang github actions on windows...
/// //let res = context.read_line(Prompt::from("[prompt]$ "), None);
/// ```
pub struct Vi {
    mode_stack: ModeStack,
    current_command: Vec<Key>,
    last_command: Vec<Key>,
    current_insert: Option<Key>,
    last_insert: Option<Key>,
    count: u32,
    secondary_count: u32,
    last_count: u32,
    movement_reset: bool,
    last_char_movement: Option<(char, CharMovement)>,
    esc_sequence: Option<(char, char, u32)>,
    last_insert_ms: u128,
    keyword_rule: Box<dyn ViKeywordRule>,
    normal_prompt_prefix: Option<String>,
    normal_prompt_suffix: Option<String>,
    insert_prompt_prefix: Option<String>,
    insert_prompt_suffix: Option<String>,
}

impl Default for Vi {
    fn default() -> Self {
        Vi {
            mode_stack: ModeStack::with_insert(),
            current_command: Vec::new(),
            last_command: Vec::new(),
            current_insert: None,
            // we start vi in insert mode
            last_insert: Some(Key::new(KeyCode::Char('i'))),
            count: 0,
            secondary_count: 0,
            last_count: 0,
            movement_reset: false,
            last_char_movement: None,
            esc_sequence: None,
            last_insert_ms: 0,
            keyword_rule: Box::new(DefaultViKeywordRule::new()),
            normal_prompt_prefix: None,
            normal_prompt_suffix: None,
            insert_prompt_prefix: None,
            insert_prompt_suffix: None,
        }
    }
}

impl Vi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_normal_prompt_prefix(&mut self, prefix: Option<String>) {
        self.normal_prompt_prefix = prefix;
    }

    pub fn set_normal_prompt_suffix(&mut self, suffix: Option<String>) {
        self.normal_prompt_suffix = suffix;
    }

    pub fn set_insert_prompt_prefix(&mut self, prefix: Option<String>) {
        self.insert_prompt_prefix = prefix;
    }

    pub fn set_insert_prompt_suffix(&mut self, suffix: Option<String>) {
        self.insert_prompt_suffix = suffix;
    }

    pub fn set_esc_sequence(&mut self, key1: char, key2: char, timeout_ms: u32) {
        self.esc_sequence = Some((key1, key2, timeout_ms));
    }

    pub fn set_keyword_rule(&mut self, keyword_rule: Box<dyn ViKeywordRule>) {
        self.keyword_rule = keyword_rule;
    }

    /// Get the current mode.
    fn mode(&self) -> Mode {
        self.mode_stack.mode()
    }

    fn set_mode<'a>(&mut self, mode: Mode, ed: &mut Editor<'a>) -> io::Result<()> {
        use self::Mode::*;
        self.set_mode_preserve_last(mode, ed)?;
        if mode == Insert {
            self.last_count = 0;
            self.last_command.clear();
        }
        Ok(())
    }

    fn set_editor_mode<'a>(&self, ed: &mut Editor<'a>) -> io::Result<()> {
        use Mode::*;
        match self.mode() {
            Insert => {
                if let Some(prefix) = &self.insert_prompt_prefix {
                    ed.set_prompt_prefix(prefix);
                } else {
                    ed.clear_prompt_prefix();
                }
                if let Some(suffix) = &self.insert_prompt_suffix {
                    ed.set_prompt_suffix(suffix);
                } else {
                    ed.clear_prompt_suffix();
                }
            }
            Normal => {
                if let Some(prefix) = &self.normal_prompt_prefix {
                    ed.set_prompt_prefix(prefix);
                } else {
                    ed.clear_prompt_prefix();
                }
                if let Some(suffix) = &self.normal_prompt_suffix {
                    ed.set_prompt_suffix(suffix);
                } else {
                    ed.clear_prompt_suffix();
                }
            }
            _ => {} // Leave the last one
        }
        ed.display_term()
    }

    fn set_mode_preserve_last<'a>(&mut self, mode: Mode, ed: &mut Editor<'a>) -> io::Result<()> {
        use self::Mode::*;

        ed.set_no_eol(mode == Normal);
        self.movement_reset = mode != Insert;
        self.mode_stack.push(mode);
        self.set_editor_mode(ed)?;

        if mode == Insert || mode == Tilde {
            ed.current_buffer_mut().start_undo_group();
        }
        Ok(())
    }

    fn pop_mode_after_movement<'a>(
        &mut self,
        move_type: MoveType,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        use self::Mode::*;
        use self::MoveType::*;

        let original_mode = self.mode_stack.pop();
        let last_mode = {
            // after popping, if mode is delete or change, pop that too. This is used for movements
            // with sub commands like 't' (MoveToChar) and 'g' (G).
            match self.mode() {
                Delete(_) | Yank(_) => self.mode_stack.pop(),
                _ => original_mode,
            }
        };

        ed.set_no_eol(self.mode() == Normal);
        self.movement_reset = self.mode() != Mode::Insert;

        if let Delete(_) | Yank(_) = last_mode {
            // perform the delete operation
            match last_mode {
                Mode::Delete(start_pos) => match move_type {
                    Exclusive => ed.delete_until(start_pos)?,
                    Inclusive => ed.delete_until_inclusive(start_pos)?,
                },
                Mode::Yank(start_pos) => match move_type {
                    Exclusive => ed.yank_until(start_pos)?,
                    Inclusive => ed.yank_until_inclusive(start_pos)?,
                },
                _ => unreachable!(),
            }

            // update the last state
            mem::swap(&mut self.last_command, &mut self.current_command);
            self.last_insert = self.current_insert;
            self.last_count = self.count;

            // reset our counts
            self.count = 0;
            self.secondary_count = 0;
        }

        // in normal mode, count goes back to 0 after movement
        if original_mode == Normal {
            self.count = 0;
        }

        self.set_editor_mode(ed)
    }

    fn pop_mode<'a>(&mut self, ed: &mut Editor<'a>) -> io::Result<()> {
        use self::Mode::*;

        let last_mode = self.mode_stack.pop();
        ed.set_no_eol(self.mode() == Normal);
        self.movement_reset = self.mode() != Insert;

        if last_mode == Insert || last_mode == Tilde {
            ed.current_buffer_mut().end_undo_group();
        }

        if last_mode == Tilde {
            ed.display_term()
        } else {
            self.set_editor_mode(ed)
        }
    }

    /// Return to normal mode.
    fn normal_mode_abort<'a>(&mut self, ed: &mut Editor<'a>) -> io::Result<()> {
        self.mode_stack.clear();
        ed.set_no_eol(true);
        self.count = 0;
        self.set_editor_mode(ed)
    }

    /// When doing a move, 0 should behave the same as 1 as far as the count goes.
    fn move_count(&self) -> usize {
        match self.count {
            0 => 1,
            _ => self.count as usize,
        }
    }

    /// Get the current count or the number of remaining chars in the buffer.
    fn move_count_left<'a>(&self, ed: &Editor<'a>) -> usize {
        cmp::min(ed.cursor(), self.move_count())
    }

    /// Get the current count or the number of remaining chars in the buffer.
    fn move_count_right<'a>(&self, ed: &Editor<'a>) -> usize {
        cmp::min(
            ed.current_buffer().num_graphemes() - ed.cursor(),
            self.move_count(),
        )
    }

    fn repeat<'a>(&mut self, ed: &mut Editor<'a>) -> io::Result<()> {
        self.last_count = self.count;
        let keys = mem::take(&mut self.last_command);

        if let Some(insert_key) = self.last_insert {
            // enter insert mode if necessary
            self.handle_key_core(insert_key, ed)?;
        }

        for k in &keys {
            self.handle_key_core(*k, ed)?;
        }

        if self.last_insert.is_some() {
            // leave insert mode
            self.handle_key_core(Key::new(KeyCode::Esc), ed)?;
        }

        // restore the last command
        self.last_command = keys;

        Ok(())
    }

    fn handle_key_common<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        match key.mods {
            Some(KeyMod::Ctrl) => {
                if key.code == KeyCode::Char('l') {
                    ed.clear()
                } else {
                    Ok(())
                }
            }
            None => match key.code {
                KeyCode::Left => ed.move_cursor_left(1),
                KeyCode::Right => ed.move_cursor_right(1),
                KeyCode::Up => ed.move_up(),
                KeyCode::Down => ed.move_down(),
                KeyCode::Home => ed.move_cursor_to_start_of_line(),
                KeyCode::End => ed.move_cursor_to_end_of_line(),
                KeyCode::Backspace => ed.delete_before_cursor(),
                KeyCode::Delete => ed.delete_after_cursor(),
                KeyCode::Null => Ok(()),
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn handle_key_insert<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        match (key.code, key.mods) {
            (KeyCode::Esc, None) | (KeyCode::Char('['), Some(KeyMod::Ctrl)) => {
                // perform any repeats
                if self.count > 0 {
                    self.last_count = self.count;
                    for _ in 1..self.count {
                        let keys = mem::take(&mut self.last_command);
                        for k in keys {
                            self.handle_key_core(k, ed)?;
                        }
                    }
                    self.count = 0;
                }
                // cursor moves to the left when switching from insert to normal mode
                ed.move_cursor_left(1)?;
                self.pop_mode(ed)
            }
            (KeyCode::Char(c), None) => {
                let in_ms = if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
                    duration.as_millis()
                } else {
                    0
                };
                if self.movement_reset {
                    ed.current_buffer_mut().end_undo_group();
                    ed.current_buffer_mut().start_undo_group();
                    self.last_command.clear();
                    self.movement_reset = false;
                    // vim behaves as if this was 'i'
                    self.last_insert = Some(Key::new(KeyCode::Char('i')));
                }
                let mut esc = false;
                if let Some((s1, s2, ms)) = self.esc_sequence {
                    if let Some(Key {
                        code: KeyCode::Char(last_c),
                        mods: None,
                    }) = self.last_command.last()
                    {
                        if *last_c == s1
                            && c == s2
                            && (in_ms > 0)
                            && (in_ms - self.last_insert_ms) < ms as u128
                        {
                            esc = true;
                        }
                    }
                }
                self.last_insert_ms = in_ms;
                if esc {
                    ed.move_cursor_left(1)?;
                    let pos = ed.cursor() + self.move_count_right(ed);
                    ed.delete_until_silent(pos)?;
                    self.handle_key_insert(Key::new(KeyCode::Esc), ed)
                } else {
                    self.last_command.push(key);
                    ed.insert_after_cursor(c)
                }
            }
            // delete and backspace need to be included in the command buffer
            (KeyCode::Backspace, None) | (KeyCode::Delete, None) => {
                if self.movement_reset {
                    ed.current_buffer_mut().end_undo_group();
                    ed.current_buffer_mut().start_undo_group();
                    self.last_command.clear();
                    self.movement_reset = false;
                    // vim behaves as if this was 'i'
                    self.last_insert = Some(Key::new(KeyCode::Char('i')));
                }
                self.last_command.push(key);
                self.handle_key_common(key, ed)
            }
            // if this is a movement while in insert mode, reset the repeat count
            (KeyCode::Left, None)
            | (KeyCode::Right, None)
            | (KeyCode::Home, None)
            | (KeyCode::End, None) => {
                self.count = 0;
                self.movement_reset = true;
                self.handle_key_common(key, ed)
            }
            // up and down require even more special handling
            (KeyCode::Up, None) => {
                self.count = 0;
                self.movement_reset = true;
                ed.current_buffer_mut().end_undo_group();
                ed.move_up()?;
                ed.current_buffer_mut().start_undo_group();
                Ok(())
            }
            (KeyCode::Down, None) => {
                self.count = 0;
                self.movement_reset = true;
                ed.current_buffer_mut().end_undo_group();
                ed.move_down()?;
                ed.current_buffer_mut().start_undo_group();
                Ok(())
            }
            _ => self.handle_key_common(key, ed),
        }
    }

    fn handle_redo<'a>(&mut self, ed: &mut Editor<'a>) -> io::Result<()> {
        let count = self.move_count();
        self.count = 0;
        for _ in 0..count {
            if let Some(cursor_pos) = ed.redo() {
                ed.move_cursor_to(cursor_pos)?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn handle_key_normal<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        use self::CharMovement::*;
        use self::Mode::*;
        use self::MoveType::*;

        match key.mods {
            Some(KeyMod::Ctrl) => match key.code {
                KeyCode::Char('r') => self.handle_redo(ed),
                _ => Ok(()),
            },
            None => {
                match key.code {
                    KeyCode::Esc => {
                        self.count = 0;
                        Ok(())
                    }
                    KeyCode::Char('i') => {
                        self.last_insert = Some(key);
                        self.set_mode(Insert, ed)
                    }
                    KeyCode::Char('a') => {
                        self.last_insert = Some(key);
                        self.set_mode(Insert, ed)?;
                        ed.move_cursor_right(1)
                    }
                    KeyCode::Char('A') => {
                        self.last_insert = Some(key);
                        self.set_mode(Insert, ed)?;
                        ed.move_cursor_to_end_of_line()
                    }
                    KeyCode::Char('I') => {
                        self.last_insert = Some(key);
                        self.set_mode(Insert, ed)?;
                        ed.move_cursor_to_start_of_line()
                    }
                    KeyCode::Char('s') => {
                        self.last_insert = Some(key);
                        self.set_mode(Insert, ed)?;
                        let pos = ed.cursor() + self.move_count_right(ed);
                        ed.delete_until(pos)?;
                        self.last_count = self.count;
                        self.count = 0;
                        Ok(())
                    }
                    KeyCode::Char('r') => self.set_mode(Mode::Replace, ed),
                    KeyCode::Char('d') | KeyCode::Char('c') | KeyCode::Char('y') => {
                        self.current_command.clear();

                        if (key.code == KeyCode::Char('d')) | (key.code == KeyCode::Char('y')) {
                            // handle special 'd'  & 'y' key stuff
                            self.current_insert = None;
                            self.current_command.push(key);
                        } else if key.code == KeyCode::Char('c') {
                            // handle special 'c' key stuff
                            self.current_insert = Some(key);
                            self.current_command.clear();
                            self.set_mode(Insert, ed)?;
                        }

                        let start_pos = ed.cursor();
                        if key.code == KeyCode::Char('y') {
                            self.set_mode(Mode::Yank(start_pos), ed)?;
                        } else {
                            self.set_mode(Mode::Delete(start_pos), ed)?;
                        }
                        self.secondary_count = self.count;
                        self.count = 0;
                        Ok(())
                    }
                    KeyCode::Char('D') => {
                        // update the last command state
                        self.last_insert = None;
                        self.last_command.clear();
                        self.last_command.push(key);
                        self.count = 0;
                        self.last_count = 0;

                        ed.delete_all_after_cursor()
                    }
                    KeyCode::Char('C') => {
                        // update the last command state
                        self.last_insert = None;
                        self.last_command.clear();
                        self.last_command.push(key);
                        self.count = 0;
                        self.last_count = 0;

                        self.set_mode_preserve_last(Insert, ed)?;
                        ed.delete_all_after_cursor()
                    }
                    KeyCode::Char('.') => {
                        // repeat the last command
                        self.count = match (self.count, self.last_count) {
                            // if both count and last_count are zero, use 1
                            (0, 0) => 1,
                            // if count is zero, use last_count
                            (0, _) => self.last_count,
                            // otherwise use count
                            (_, _) => self.count,
                        };
                        self.repeat(ed)
                    }
                    KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
                        let count = self.move_count_left(ed);
                        ed.move_cursor_left(count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => {
                        let count = self.move_count_right(ed);
                        ed.move_cursor_right(count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        ed.move_up()?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        ed.move_down()?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('t') => self.set_mode(Mode::MoveToChar(RightUntil), ed),
                    KeyCode::Char('T') => self.set_mode(Mode::MoveToChar(LeftUntil), ed),
                    KeyCode::Char('f') => self.set_mode(Mode::MoveToChar(RightAt), ed),
                    KeyCode::Char('F') => self.set_mode(Mode::MoveToChar(LeftAt), ed),
                    KeyCode::Char(';') => self.handle_key_move_to_char(key, Repeat, ed),
                    KeyCode::Char(',') => self.handle_key_move_to_char(key, ReverseRepeat, ed),
                    KeyCode::Char('w') => {
                        let count = self.move_count();
                        self.move_word(ed, count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('W') => {
                        let count = self.move_count();
                        self.move_word_ws(ed, count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('e') => {
                        let count = self.move_count();
                        self.move_to_end_of_word(ed, count)?;
                        self.pop_mode_after_movement(Inclusive, ed)
                    }
                    KeyCode::Char('E') => {
                        let count = self.move_count();
                        self.move_to_end_of_word_ws(ed, count)?;
                        self.pop_mode_after_movement(Inclusive, ed)
                    }
                    KeyCode::Char('b') => {
                        let count = self.move_count();
                        self.move_word_back(ed, count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('B') => {
                        let count = self.move_count();
                        self.move_word_ws_back(ed, count)?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('g') => self.set_mode(Mode::G, ed),
                    // if count is 0, 0 should move to start of line
                    KeyCode::Char('0') if self.count == 0 => {
                        ed.move_cursor_to_start_of_line()?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char(i @ '0'..='9') => {
                        if let Some(i) = i.to_digit(10) {
                            // count = count * 10 + i
                            self.count = self.count.saturating_mul(10).saturating_add(i);
                        }
                        Ok(())
                    }
                    KeyCode::Char('^') => {
                        ed.move_cursor_to_start_of_line()?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('$') => {
                        ed.move_cursor_to_end_of_line()?;
                        self.pop_mode_after_movement(Exclusive, ed)
                    }
                    KeyCode::Char('x') | KeyCode::Delete => {
                        // update the last command state
                        self.last_insert = None;
                        self.last_command.clear();
                        self.last_command.push(key);
                        self.last_count = self.count;

                        let pos = ed.cursor() + self.move_count_right(ed);
                        ed.delete_until(pos)?;
                        self.count = 0;
                        Ok(())
                    }
                    KeyCode::Char('~') => {
                        // update the last command state
                        self.last_insert = None;
                        self.last_command.clear();
                        self.last_command.push(key);
                        self.last_count = self.count;

                        self.set_mode(Tilde, ed)?;
                        for _ in 0..self.move_count_right(ed) {
                            ed.flip_case()?;
                        }
                        self.pop_mode(ed)?;
                        Ok(())
                    }
                    KeyCode::Char('u') => {
                        let count = self.move_count();
                        self.count = 0;
                        for _ in 0..count {
                            if let Some(cursor_pos) = ed.undo() {
                                ed.move_cursor_to(cursor_pos)?;
                            } else {
                                break;
                            }
                        }
                        Ok(())
                    }
                    KeyCode::Char('p') => {
                        let count = self.move_count();
                        self.count = 0;
                        ed.paste(true, count)
                    }
                    KeyCode::Char('P') => {
                        let count = self.move_count();
                        self.count = 0;
                        ed.paste(false, count)
                    }
                    _ => self.handle_key_common(key, ed),
                }
            }
            _ => Ok(()),
        }
    }

    fn handle_key_replace<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        match key.code {
            KeyCode::Char(c) => {
                // make sure there are enough chars to replace
                if self.move_count_right(ed) == self.move_count() {
                    // update the last command state
                    self.last_insert = None;
                    self.last_command.clear();
                    self.last_command.push(Key::new(KeyCode::Char('r')));
                    self.last_command.push(key);
                    self.last_count = self.count;

                    // replace count characters
                    ed.current_buffer_mut().start_undo_group();
                    for _ in 0..self.move_count_right(ed) {
                        ed.delete_after_cursor()?;
                        ed.insert_after_cursor(c)?;
                    }
                    ed.current_buffer_mut().end_undo_group();

                    ed.move_cursor_left(1)?;
                }
                self.pop_mode(ed)?;
            }
            // not a char
            _ => {
                self.normal_mode_abort(ed)?;
            }
        };

        // back to normal mode
        self.count = 0;
        Ok(())
    }

    fn set_count(&mut self) {
        // set count
        self.count = match (self.count, self.secondary_count) {
            (0, 0) => 0,
            (_, 0) => self.count,
            (0, _) => self.secondary_count,
            _ => {
                // secondary_count * count
                self.secondary_count.saturating_mul(self.count)
            }
        };
    }

    fn handle_key_delete_change_yank<'a>(
        &mut self,
        key: Key,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        match (
            key,
            TextObjectMode::from_key_code(key.code),
            self.current_insert,
        ) {
            // check if this is a movement key
            (key, Some(text_object), _) if key.mods == None => {
                self.current_command.push(key);
                self.current_insert = None;
                self.set_mode(Mode::TextObject(text_object), ed)?;
                Ok(())
            }
            (key, _, _)
                if is_movement_key(key)
                    | (key.code == KeyCode::Char('0') && key.mods == None && self.count == 0) =>
            {
                self.set_count();

                // update the last command state
                self.current_command.push(key);

                match (self.mode(), is_movement_key_to_right(key)) {
                    // in vim, movement to the right in yank mode does not cause the cursor to move
                    (Mode::Yank(_), true) => {
                        let before = ed.cursor();
                        self.handle_key_normal(key, ed)?;
                        ed.move_cursor_to(before)
                    }
                    // execute movement
                    (_, _) => self.handle_key_normal(key, ed),
                }
            }
            // handle numeric keys
            (
                Key {
                    code: KeyCode::Char('0'..='9'),
                    mods: None,
                },
                _,
                _,
            ) => self.handle_key_normal(key, ed),
            (
                Key {
                    code: KeyCode::Char('c'),
                    mods: None,
                },
                _,
                Some(Key {
                    code: KeyCode::Char('c'),
                    mods: None,
                }),
            )
            | (
                Key {
                    code: KeyCode::Char('d'),
                    mods: None,
                },
                _,
                None,
            )
            | (
                Key {
                    code: KeyCode::Char('y'),
                    mods: None,
                },
                _,
                None,
            ) => {
                // updating the last command buffer doesn't really make sense in this context.
                // Repeating 'dd' will simply erase and already erased line. Any other commands
                // will then become the new last command and the user will need to press 'dd' again
                // to clear the line. The same largely applies to the 'cc' command. We update the
                // last command here anyway ??\_(???)_/??
                self.current_command.push(key);

                // delete or yank the whole line
                self.count = 0;
                self.secondary_count = 0;
                ed.move_cursor_to_start_of_line()?;
                if key.code == KeyCode::Char('y') {
                    ed.yank_all_after_cursor()?;
                } else {
                    ed.delete_all_after_cursor()?;
                }

                // return to the previous mode
                self.pop_mode(ed)
            }
            // not a delete or change command, back to normal mode
            _ => self.normal_mode_abort(ed),
        }
    }

    fn handle_key_move_to_char<'a>(
        &mut self,
        key: Key,
        movement: CharMovement,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        use self::CharMovement::*;
        use self::MoveType::*;

        let count = self.move_count();
        self.count = 0;

        let (key_code, movement) = match (key, movement, self.last_char_movement) {
            // repeat the last movement
            (_, Repeat, Some((c, last_movement))) => (KeyCode::Char(c), last_movement),
            // repeat the last movement in the opposite direction
            (_, ReverseRepeat, Some((c, LeftUntil))) => (KeyCode::Char(c), RightUntil),
            (_, ReverseRepeat, Some((c, RightUntil))) => (KeyCode::Char(c), LeftUntil),
            (_, ReverseRepeat, Some((c, LeftAt))) => (KeyCode::Char(c), RightAt),
            (_, ReverseRepeat, Some((c, RightAt))) => (KeyCode::Char(c), LeftAt),
            // repeat with no last_char_movement, invalid
            (_, Repeat, None) | (_, ReverseRepeat, None) => {
                return self.normal_mode_abort(ed);
            }
            // pass valid keys through as is
            (
                Key {
                    code: KeyCode::Char(c),
                    mods: None,
                },
                _,
                _,
            ) => {
                // store last command info
                self.last_char_movement = Some((c, movement));
                self.current_command.push(key);
                (key.code, movement)
            }
            // all other combinations are invalid, abort
            _ => {
                return self.normal_mode_abort(ed);
            }
        };

        match key_code {
            KeyCode::Char(c) => {
                let move_type;
                let mut return_to_pos = None;
                match movement {
                    RightUntil => {
                        move_type = Inclusive;
                        match find_char(ed.current_buffer(), ed.cursor() + 1, c, count) {
                            Some(i) => {
                                let prev_mode = self.mode_stack.pop();
                                return_to_pos = match self.mode() {
                                    Mode::Yank(_) => {
                                        ed.yank_until(ed.cursor())?;
                                        Some(ed.cursor())
                                    }
                                    _ => None,
                                };
                                self.set_mode_preserve_last(prev_mode, ed)?;
                                ed.move_cursor_to(i - 1)
                            }
                            None => Ok(()),
                        }
                    }
                    RightAt => {
                        move_type = Inclusive;
                        match find_char(ed.current_buffer(), ed.cursor() + 1, c, count) {
                            Some(i) => {
                                let prev_mode = self.mode_stack.pop();
                                return_to_pos = match self.mode() {
                                    Mode::Yank(_) => {
                                        ed.yank_until(ed.cursor())?;
                                        Some(ed.cursor())
                                    }
                                    _ => None,
                                };
                                self.set_mode_preserve_last(prev_mode, ed)?;
                                ed.move_cursor_to(i)
                            }
                            None => Ok(()),
                        }
                    }
                    LeftUntil => {
                        move_type = Exclusive;
                        match find_char_rev(ed.current_buffer(), ed.cursor(), c, count) {
                            Some(i) => ed.move_cursor_to(i + 1),
                            None => Ok(()),
                        }
                    }
                    LeftAt => {
                        move_type = Exclusive;
                        match find_char_rev(ed.current_buffer(), ed.cursor(), c, count) {
                            Some(i) => ed.move_cursor_to(i),
                            None => Ok(()),
                        }
                    }
                    Repeat | ReverseRepeat => unreachable!(),
                }?;

                let result = self.pop_mode_after_movement(move_type, ed);
                if let Some(pos) = return_to_pos {
                    // in vim, movement to the right in yank mode does not cause the cursor to move
                    ed.move_cursor_to(pos)?;
                }
                result
            }

            // can't get here due to our match above
            _ => unreachable!(),
        }
    }

    fn handle_key_g<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        use self::MoveType::*;

        let count = self.move_count();
        self.current_command.push(key);

        let res = match key.code {
            KeyCode::Char('e') => {
                self.move_to_end_of_word_back(ed, count)?;
                self.pop_mode_after_movement(Inclusive, ed)
            }
            KeyCode::Char('E') => {
                self.move_to_end_of_word_ws_back(ed, count)?;
                self.pop_mode_after_movement(Inclusive, ed)
            }

            // not a supported command
            _ => self.normal_mode_abort(ed),
        };

        self.count = 0;
        res
    }

    fn reset_curor_pos_for_command_mode(&mut self, pos: usize) -> Option<Mode> {
        match self.mode_stack.pop() {
            Mode::Delete(_) => {
                self.mode_stack.push(Mode::Delete(pos));
            }
            Mode::Yank(_) => {
                self.mode_stack.push(Mode::Yank(pos));
            }
            // Delete and Yank are the only supported modes. They are the only command objects
            // that currently work with text objects.
            _ => return None,
        }
        Some(self.mode())
    }

    fn handle_key_text_object<'a>(
        &mut self,
        key: Key,
        text_object: TextObjectMode,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        self.pop_mode(ed)?;

        self.set_count();

        match (TextObjectMovement::from_key_code(key.code), key.mods) {
            (Some(movement), None) => match movement {
                TextObjectMovement::Word => self.handle_text_object_movement_word(text_object, ed),
                TextObjectMovement::Surround(beg, end) => {
                    self.handle_text_object_movement_surround(text_object, beg, end, ed)
                }
            },
            (_, _) => self.normal_mode_abort(ed),
        }
    }

    fn handle_text_object_movement_surround<'a>(
        &mut self,
        text_object: TextObjectMode,
        beg: char,
        end: char,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        let count = self.move_count();

        if let Some(curr_char) = ed.curr_char() {
            let mut behind = None;
            let mut ahead = None;
            let start = ed.cursor();
            let buf = ed.current_buffer();
            // if the beg and end chars are different, e.g. beg != end
            // (parentheses, brackets, and braces but not quotes, backticks etc.)
            // then balance matters and matches must be excluded if they are
            // matched. For example the vim sequence 'di(' (cursor is indicated
            // by bars) applied on the following string:
            //     "(aaaa)b|b|bb)"
            // does not result in a deletion because the open parens at position
            // 0 matches the close parentheses at position 5 and is thus
            // precluded from matching the close parens to the right of the
            // cursor. For analogous reasons 'di(' does nothing to this string:
            //     "(aa|a|a(bbbb)cccc"
            // To ensure this behavior, balancing logic for beg, and end must be applied.
            if curr_char.eq(&end.to_string()) {
                let is_behind = if beg != end {
                    find_char_rev_balance_delim(buf, start, beg, end, count)
                } else {
                    find_char_rev(buf, start, beg, count)
                };
                if is_behind.is_some() {
                    behind = is_behind;
                    ahead = Some(start);
                }
            } else if curr_char.eq(&beg.to_string()) {
                let is_ahead = if beg != end {
                    find_char_balance_delim(buf, start + 1, end, beg, count)
                } else {
                    find_char(buf, start, end, count)
                };
                if is_ahead.is_some() {
                    behind = Some(start);
                    ahead = is_ahead;
                }
            } else if beg != end {
                behind = find_char_rev_balance_delim(buf, start, beg, end, count);
                ahead = find_char_balance_delim(buf, start, end, beg, count);
            } else {
                behind = find_char_rev(buf, start, beg, count);
                ahead = find_char(buf, start, end, count);
            }
            if let (Some(r_idx), Some(f_idx)) = (behind, ahead) {
                let (mode, move_type) = match text_object {
                    TextObjectMode::Whole => {
                        let mode = self.reset_curor_pos_for_command_mode(r_idx);
                        ed.move_cursor_to(f_idx)?;
                        (mode, MoveType::Inclusive)
                    }
                    TextObjectMode::Inner => {
                        let mode = self.reset_curor_pos_for_command_mode(r_idx + 1);
                        ed.move_cursor_to(f_idx)?;
                        (mode, MoveType::Exclusive)
                    }
                };
                match mode {
                    Some(mode) => {
                        // match move_type { }
                        self.pop_mode_after_movement(move_type, ed)?;
                        if let Mode::Yank(before) = mode {
                            ed.move_cursor_to(before)
                        } else {
                            Ok(())
                        }
                    }
                    None => self.normal_mode_abort(ed),
                }
            } else {
                self.normal_mode_abort(ed)
            }
        } else {
            self.normal_mode_abort(ed)
        }
    }

    fn handle_text_object_movement_word<'a>(
        &mut self,
        text_object: TextObjectMode,
        ed: &mut Editor<'a>,
    ) -> io::Result<()> {
        let count = self.move_count();
        if !ed.is_cursor_at_beginning_of_word_or_line() {
            match text_object {
                TextObjectMode::Whole => self.move_word_ws_back(ed, 1)?,
                TextObjectMode::Inner => self.move_word_back(ed, 1)?,
            }
        }
        let mode = self.reset_curor_pos_for_command_mode(ed.cursor());
        match mode {
            Some(mode) => {
                let move_type = match text_object {
                    TextObjectMode::Whole => {
                        self.move_word(ed, count)?;
                        MoveType::Exclusive
                    }
                    TextObjectMode::Inner => {
                        if self.count > 1 {
                            self.move_word_ws_is_word(ed, count)?;
                        } else {
                            self.move_to_end_of_word(ed, count)?;
                        }
                        MoveType::Inclusive
                    }
                };
                self.pop_mode_after_movement(move_type, ed)?;
                if let Mode::Yank(before) = mode {
                    ed.move_cursor_to(before)
                } else {
                    Ok(())
                }
            }
            None => self.normal_mode_abort(ed),
        }
    }

    fn move_word_ws_is_word(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word(ed, ViMoveMode::Keyword, ViMoveDir::Right, count, true)
    }

    fn move_word(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word(ed, ViMoveMode::Keyword, ViMoveDir::Right, count, false)
    }

    fn move_word_ws(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word(ed, ViMoveMode::Whitespace, ViMoveDir::Right, count, false)
    }

    fn move_to_end_of_word_back(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word(ed, ViMoveMode::Keyword, ViMoveDir::Left, count, false)
    }

    fn move_to_end_of_word_ws_back(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word(ed, ViMoveMode::Whitespace, ViMoveDir::Left, count, false)
    }

    fn vi_move_word(
        &self,
        ed: &mut Editor,
        move_mode: ViMoveMode,
        direction: ViMoveDir,
        count: usize,
        ws_included_in_count: bool,
    ) -> io::Result<()> {
        #[derive(Clone, Copy)]
        enum State {
            Whitespace,
            Keyword,
            NonKeyword,
        }

        let mut cursor = ed.cursor();
        'repeat: for _ in 0..count {
            let buf = ed.current_buffer();
            let mut state = match buf.grapheme_after(cursor) {
                None => break,
                Some(str) => match str {
                    str if str.trim().is_empty() => State::Whitespace,
                    str if self.keyword_rule.is_vi_keyword(str) => State::Keyword,
                    _ => State::NonKeyword,
                },
            };

            while direction.advance(&mut cursor, buf.num_graphemes()) {
                let str = match buf.grapheme_after(cursor) {
                    Some(str) => str,
                    _ => break 'repeat,
                };

                // if ws_included_in_count is true we want to make sure we treat
                // any contiguous string of whitespace appropriately towards
                // the overall count, this means that at (NonKeyWord and Keyword)
                // to Whitespace boundaries we need to break so the count loop
                // increments one more time. The default behavior just cycles
                // through Whitespace.
                match state {
                    State::Whitespace => match str {
                        str if str.trim().is_empty() => {}
                        _ => {
                            break;
                        }
                    },
                    State::Keyword => match str {
                        str if str.trim().is_empty() => {
                            if ws_included_in_count {
                                break;
                            } else {
                                state = State::Whitespace
                            }
                        }
                        str if move_mode == ViMoveMode::Keyword
                            && !self.keyword_rule.is_vi_keyword(str) =>
                        {
                            break
                        }
                        _ => {}
                    },
                    State::NonKeyword => match str {
                        str if str.trim().is_empty() => {
                            if ws_included_in_count {
                                break;
                            } else {
                                state = State::Whitespace
                            }
                        }
                        str if move_mode == ViMoveMode::Keyword
                            && self.keyword_rule.is_vi_keyword(str) =>
                        {
                            break
                        }
                        _ => {}
                    },
                }
            }
        }

        // default positioning of cursor when moving in this manner ends one
        // position to the left of the desired positioning in vi when moving
        // and treating whitespace as words for the purposed of text objects.
        if ws_included_in_count && count > 0 {
            cursor -= 1;
        }
        ed.move_cursor_to(cursor)
    }

    fn move_to_end_of_word(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word_end(ed, ViMoveMode::Keyword, ViMoveDir::Right, count)
    }

    fn move_to_end_of_word_ws(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word_end(ed, ViMoveMode::Whitespace, ViMoveDir::Right, count)
    }

    fn move_word_back(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word_end(ed, ViMoveMode::Keyword, ViMoveDir::Left, count)
    }

    fn move_word_ws_back(&self, ed: &mut Editor, count: usize) -> io::Result<()> {
        self.vi_move_word_end(ed, ViMoveMode::Whitespace, ViMoveDir::Left, count)
    }

    fn vi_move_word_end(
        &self,
        ed: &mut Editor,
        move_mode: ViMoveMode,
        direction: ViMoveDir,
        count: usize,
    ) -> io::Result<()> {
        enum State {
            Whitespace,
            EndOnWord,
            EndOnOther,
            EndOnWhitespace,
        }

        let mut cursor = ed.cursor();
        'repeat: for _ in 0..count {
            let buf = ed.current_buffer();
            let mut state = State::Whitespace;

            while direction.advance(&mut cursor, buf.num_graphemes()) {
                let str = match buf.grapheme_after(cursor) {
                    Some(c) => c,
                    _ => break 'repeat,
                };

                match state {
                    State::Whitespace => match str {
                        // skip initial whitespace
                        str if str.trim().is_empty() => {}
                        // if we are in keyword mode and found a keyword, stop on word
                        str if move_mode == ViMoveMode::Keyword
                            && self.keyword_rule.is_vi_keyword(str) =>
                        {
                            state = State::EndOnWord;
                        }
                        // not in keyword mode, stop on whitespace
                        _ if move_mode == ViMoveMode::Whitespace => {
                            state = State::EndOnWhitespace;
                        }
                        // in keyword mode, found non-whitespace non-keyword, stop on anything
                        _ => {
                            state = State::EndOnOther;
                        }
                    },
                    State::EndOnWord if !self.keyword_rule.is_vi_keyword(str) => {
                        direction.go_back(&mut cursor, buf.num_graphemes());
                        break;
                    }
                    State::EndOnWhitespace if str.trim().is_empty() => {
                        direction.go_back(&mut cursor, buf.num_graphemes());
                        break;
                    }
                    State::EndOnOther
                        if str.trim().is_empty() || self.keyword_rule.is_vi_keyword(str) =>
                    {
                        direction.go_back(&mut cursor, buf.num_graphemes());
                        break;
                    }
                    _ => {}
                }
            }
        }

        ed.move_cursor_to(cursor)
    }
}

impl KeyMap for Vi {
    fn handle_key_core<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        match self.mode() {
            Mode::Normal => self.handle_key_normal(key, ed),
            Mode::Insert => self.handle_key_insert(key, ed),
            Mode::Replace => self.handle_key_replace(key, ed),
            Mode::Delete(_) | Mode::Yank(_) => self.handle_key_delete_change_yank(key, ed),
            Mode::MoveToChar(movement) => self.handle_key_move_to_char(key, movement, ed),
            Mode::G => self.handle_key_g(key, ed),
            Mode::TextObject(prev) => self.handle_key_text_object(key, prev, ed),
            Mode::Tilde => unreachable!(),
        }
    }

    fn init<'a>(&mut self, ed: &mut Editor<'a>) {
        self.mode_stack.clear();
        self.mode_stack.push(Mode::Insert);
        self.current_command.clear();
        self.last_command.clear();
        self.current_insert = None;
        // we start vi in insert mode
        self.last_insert = Some(Key::new(KeyCode::Char('i')));
        self.count = 0;
        self.secondary_count = 0;
        self.last_count = 0;
        self.movement_reset = false;
        self.last_char_movement = None;
        // since we start in insert mode, we need to start an undo group
        ed.current_buffer_mut().start_undo_group();
        let _ = self.set_editor_mode(ed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Buffer, Completer, DefaultEditorRules, Editor, History, KeyMap, Prompt};

    fn simulate_key_codes<'a, 'b, M: KeyMap, I>(
        keymap: &mut M,
        ed: &mut Editor<'a>,
        keys: I,
    ) -> bool
    where
        I: IntoIterator<Item = &'b KeyCode>,
    {
        for k in keys {
            if keymap
                .handle_key(
                    Key {
                        code: *k,
                        mods: None,
                    },
                    ed,
                    &mut EmptyCompleter,
                )
                .unwrap()
            {
                return true;
            }
        }

        false
    }

    fn simulate_keys<'a, 'b, M: KeyMap, I>(keymap: &mut M, ed: &mut Editor<'a>, keys: I) -> bool
    where
        I: IntoIterator<Item = &'b Key>,
    {
        for k in keys {
            if keymap.handle_key(*k, ed, &mut EmptyCompleter).unwrap() {
                return true;
            }
        }

        false
    }

    struct EmptyCompleter;

    impl Completer for EmptyCompleter {
        fn completions(&mut self, _start: &str) -> Vec<String> {
            Vec::default()
        }
    }

    #[test]
    fn enter_is_done() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("done").unwrap();
        assert_eq!(ed.cursor(), 4);

        assert!(simulate_keys(
            &mut map,
            &mut ed,
            [Key::new(KeyCode::Char('\n')),].iter()
        ));

        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "done");
    }

    #[test]
    fn move_cursor_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("let").unwrap();
        assert_eq!(ed.cursor(), 3);

        simulate_keys(
            &mut map,
            &mut ed,
            [Key::new(KeyCode::Left), Key::new(KeyCode::Char('f'))].iter(),
        );

        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "left");
    }

    #[test]
    fn cursor_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new(KeyCode::Left),
                Key::new(KeyCode::Left),
                Key::new(KeyCode::Right),
            ]
            .iter(),
        );

        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    fn move_cursor_start_end() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        let test_str = "let there be tests";
        ed.insert_str_after_cursor(test_str).unwrap();
        assert_eq!(ed.cursor(), test_str.len());

        simulate_keys(
            &mut map,
            &mut ed,
            [Key::new(KeyCode::Esc), Key::new(KeyCode::Char('^'))].iter(),
        );
        assert_eq!(ed.cursor(), 0);

        simulate_keys(&mut map, &mut ed, [Key::new(KeyCode::Char('^'))].iter());
        assert_eq!(ed.cursor(), 0);

        simulate_keys(&mut map, &mut ed, [Key::new(KeyCode::Char('$'))].iter());
        assert_eq!(ed.cursor(), test_str.len() - 1);

        simulate_keys(&mut map, &mut ed, [Key::new(KeyCode::Char('$'))].iter());
        assert_eq!(ed.cursor(), test_str.len() - 1);
    }

    #[test]
    fn vi_initial_insert() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new(KeyCode::Char('i')),
                Key::new(KeyCode::Char('n')),
                Key::new(KeyCode::Char('s')),
                Key::new(KeyCode::Char('e')),
                Key::new(KeyCode::Char('r')),
                Key::new(KeyCode::Char('t')),
            ]
            .iter(),
        );

        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "insert");
    }

    #[test]
    fn vi_left_right_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Left].iter());
        assert_eq!(ed.cursor(), 3);
        simulate_key_codes(&mut map, &mut ed, [KeyCode::Right].iter());
        assert_eq!(ed.cursor(), 4);

        // switching from insert mode moves the cursor left
        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc, KeyCode::Left].iter());
        assert_eq!(ed.cursor(), 2);
        simulate_key_codes(&mut map, &mut ed, [KeyCode::Right].iter());
        assert_eq!(ed.cursor(), 3);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Char('h')].iter());
        assert_eq!(ed.cursor(), 2);
        simulate_key_codes(&mut map, &mut ed, [KeyCode::Char('l')].iter());
        assert_eq!(ed.cursor(), 3);
    }

    #[test]
    /// Shouldn't be able to move past the last char in vi normal mode
    fn vi_no_eol() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc].iter());
        assert_eq!(ed.cursor(), 3);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Right, KeyCode::Right].iter());
        assert_eq!(ed.cursor(), 3);

        // in insert mode, we can move past the last char, but no further
        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Char('i'), KeyCode::Right, KeyCode::Right].iter(),
        );
        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    /// Cursor moves left when exiting insert mode.
    fn vi_switch_from_insert() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc].iter());
        assert_eq!(ed.cursor(), 3);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new(KeyCode::Char('i')),
                Key::new(KeyCode::Esc),
                Key::new(KeyCode::Char('i')),
                //Ctrl+[ is the same as escape
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('i')),
                Key::new(KeyCode::Esc),
                Key::new(KeyCode::Char('i')),
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    fn vi_normal_history_cursor_eol() {
        let mut history = History::new();
        history.push("data hostory").unwrap();
        history.push("data history").unwrap();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Up].iter());
        assert_eq!(ed.cursor(), 12);

        // in normal mode, make sure we don't end up past the last char
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Up),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 11);
    }

    #[test]
    fn vi_normal_history() {
        let mut history = History::new();
        history.push("data second").unwrap();
        history.push("skip1").unwrap();
        history.push("data one").unwrap();
        history.push("skip2").unwrap();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Up].iter());
        assert_eq!(ed.cursor(), 8);

        // in normal mode, make sure we don't end up past the last char
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('k')),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 10);
    }

    #[test]
    fn vi_search_history() {
        // Test incremental search as well as vi binding in search mode.
        let mut history = History::new();
        history.push("data pat second").unwrap();
        history.push("skip1").unwrap();
        history.push("data pat one").unwrap();
        history.push("skip2").unwrap();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("pat").unwrap();
        assert_eq!(ed.cursor(), 3);
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('r'), KeyMod::Ctrl),
                Key::new(KeyCode::Right),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 12);

        ed.delete_all_before_cursor().unwrap();
        assert_eq!(ed.cursor(), 0);
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('r'), KeyMod::Ctrl),
                Key::new(KeyCode::Char('p')),
                Key::new(KeyCode::Char('a')),
                Key::new(KeyCode::Char('t')),
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('k')),
                Key::new_mod(KeyCode::Char('f'), KeyMod::Ctrl),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 14);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('u')),
                Key::new(KeyCode::Char('i')),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('s'), KeyMod::Ctrl),
                Key::new(KeyCode::Char('p')),
                Key::new(KeyCode::Char('a')),
                Key::new(KeyCode::Char('t')),
                Key::new_mod(KeyCode::Char('f'), KeyMod::Ctrl),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 15);

        ed.delete_all_before_cursor().unwrap();
        assert_eq!(ed.cursor(), 0);
        ed.insert_str_after_cursor("pat").unwrap();
        assert_eq!(ed.cursor(), 3);
        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('s'), KeyMod::Ctrl),
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('j')),
                Key::new(KeyCode::Right),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 11);
    }

    #[test]
    fn vi_normal_delete() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Delete,
                KeyCode::Char('x'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "ta");
    }

    #[test]
    fn vi_change_with_text_objects() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);
        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "hi data data");
    }

    #[test]
    fn vi_change_paste_with_text_objects() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Esc,
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "hidata data data");
    }

    #[test]
    fn vi_delete_paste_with_text_objects() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), " datadata data");
    }

    #[test]
    fn vi_delete_with_text_objects() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('b'),
                KeyCode::Char('d'),
                KeyCode::Char('a'),
                KeyCode::Char('w'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "data data");
    }

    #[test]
    fn vi_delete_with_multi_paste() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('b'),
                KeyCode::Char('d'),
                KeyCode::Char('w'),
                KeyCode::Char('2'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 15);
        assert_eq!(String::from(ed), "data ddata data ata");
    }

    #[test]
    fn vi_delete_with_multi_paste_backwards() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('b'),
                KeyCode::Char('d'),
                KeyCode::Char('w'),
                KeyCode::Char('4'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 24);
        assert_eq!(String::from(ed), "data data data data data data");
    }

    #[test]
    fn vi_yank_paste_with_text_objects() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('w'),
                KeyCode::Char('y'),
                KeyCode::Char('a'),
                KeyCode::Char('w'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 9);
        assert_eq!(String::from(ed), "data data data data");
    }

    #[test]
    fn vi_2delete_paste_with_text_object_aw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('a'),
                KeyCode::Char('w'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 9);
        assert_eq!(String::from(ed), "data data data");
    }

    #[test]
    fn vi_2delete_paste_with_text_object_iw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "data data data");
    }

    #[test]
    fn vi_2change_paste_with_text_object_aw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('c'),
                KeyCode::Char('a'),
                KeyCode::Char('w'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Esc,
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 10);
        assert_eq!(String::from(ed), "hdata data idata");
    }

    #[test]
    fn vi_2change_paste_with_text_object_iw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('c'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Esc,
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "hidata data data");
    }

    #[test]
    fn vi_3yank_paste_with_text_object_aw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('y'),
                KeyCode::Char('a'),
                KeyCode::Char('w'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 13);
        assert_eq!(String::from(ed), "data data datadata data data");
    }

    #[test]
    fn vi_2yank_paste_with_text_object_iw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('y'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "ddata ata data data");
    }

    #[test]
    fn vi_4yank_paste_with_text_object_iw() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data data").unwrap();
        assert_eq!(ed.cursor(), 14);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('4'),
                KeyCode::Char('y'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 9);
        assert_eq!(String::from(ed), "data data data data data");
    }

    #[test]
    fn vi_delete_paste_multi_key_esc_sequence() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        map.set_esc_sequence('j', 'k', 1000u32);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Delete,
                KeyCode::Char('x'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('i'),
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "taaaaaa");
    }

    #[test]
    fn vi_delete_paste() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Delete,
                KeyCode::Char('x'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "taaaa");
    }

    #[test]
    fn vi_yank_right() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('y'),
                KeyCode::Right,
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "ddata");
    }

    #[test]
    fn vi_yank_2h() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('y'),
                KeyCode::Char('2'),
                KeyCode::Char('h'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "datata");
    }

    #[test]
    fn vi_2d_l() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('l'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "data");
    }

    #[test]
    fn vi_yank_h() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('y'),
                KeyCode::Char('h'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "datta");
    }

    #[test]
    fn vi_yank_upper_f() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('y'),
                KeyCode::Char('F'),
                KeyCode::Char('d'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "ddatata");
    }

    #[test]
    fn vi_yank_f() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('y'),
                KeyCode::Char('f'),
                KeyCode::Char('t'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "ddatata");
    }

    #[test]
    fn vi_yank_t() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('y'),
                KeyCode::Char('t'),
                KeyCode::Char('t'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "ddaata");
    }

    #[test]
    fn vi_yank_upper_t() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('y'),
                KeyCode::Char('T'),
                KeyCode::Char('d'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "daatta");
    }

    #[test]
    fn vi_yank_e() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('y'),
                KeyCode::Char('e'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "ddataata");
    }

    #[test]
    fn vi_change_paste_backward() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("some data in the buffer")
            .unwrap();
        assert_eq!(ed.cursor(), 23);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('1'),
                KeyCode::Char('9'),
                KeyCode::Char('h'),
                KeyCode::Char('2'),
                KeyCode::Char('c'),
                KeyCode::Char('f'),
                KeyCode::Char(' '),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Esc,
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 12);
        assert_eq!(String::from(ed), "sompppe data in the buffer");
    }

    #[test]
    fn vi_delete_paste_backward() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Delete,
                KeyCode::Char('x'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('0'),
                KeyCode::Char('x'),
                KeyCode::Char('P'),
                KeyCode::Char('P'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "tttaaaa");
    }

    #[test]
    fn vi_delete_paste_words() {
        let mut history = History::new();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("some data in the buffer")
            .unwrap();
        assert_eq!(ed.cursor(), 23);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('1'),
                KeyCode::Char('9'),
                KeyCode::Char('h'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('f'),
                KeyCode::Char(' '),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 24);
        assert_eq!(String::from(ed), "somie data e data e data n the buffer");
    }

    #[test]
    fn vi_delete_paste_words_reverse() {
        {
            let mut history = History::new();
            let mut out = Vec::new();
            let mut buf = String::with_capacity(512);
            let rules = DefaultEditorRules::default();
            let mut ed = Editor::new(
                &mut out,
                Prompt::from("prompt"),
                None,
                &mut history,
                &mut buf,
                &rules,
            )
            .unwrap();
            let mut map = Vi::new();
            map.init(&mut ed);
            ed.insert_str_after_cursor("some data in the buffer")
                .unwrap();
            assert_eq!(ed.cursor(), 23);

            simulate_key_codes(
                &mut map,
                &mut ed,
                [
                    KeyCode::Esc,
                    KeyCode::Char('1'),
                    KeyCode::Char('9'),
                    KeyCode::Char('h'),
                    KeyCode::Char('2'),
                    KeyCode::Char('d'),
                    KeyCode::Char('f'),
                    KeyCode::Char(' '),
                    KeyCode::Char('P'),
                    KeyCode::Char('P'),
                    KeyCode::Char('P'),
                ]
                .iter(),
            );
            assert_eq!(ed.cursor(), 21);
            assert_eq!(String::from(ed), "some datae datae data   in the buffer");
        }
    }

    #[test]
    fn vi_substitute_command() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                //ctrl+[ is the same as KeyCode::Esc
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('0')),
                Key::new(KeyCode::Char('s')),
                Key::new(KeyCode::Char('s')),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "sata");
    }

    #[test]
    fn substitute_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data").unwrap();
        assert_eq!(ed.cursor(), 4);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('s'),
                KeyCode::Char('b'),
                KeyCode::Char('e'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "beta");
    }

    #[test]
    fn substitute_with_count_repeat() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("data data").unwrap();

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new(KeyCode::Esc),
                Key::new(KeyCode::Char('0')),
                Key::new(KeyCode::Char('2')),
                Key::new(KeyCode::Char('s')),
                Key::new(KeyCode::Char('b')),
                Key::new(KeyCode::Char('e')),
                //The same as KeyCode::Esc
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('4')),
                Key::new(KeyCode::Char('l')),
                Key::new(KeyCode::Char('.')),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "beta beta");
    }

    #[test]
    /// make sure our count is accurate
    fn vi_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc].iter());
        assert_eq!(map.count, 0);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Char('1')].iter());
        assert_eq!(map.count, 1);

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Char('1')].iter());
        assert_eq!(map.count, 11);

        // switching to insert mode and back to edit mode should reset the count
        simulate_key_codes(&mut map, &mut ed, [KeyCode::Char('i'), KeyCode::Esc].iter());
        assert_eq!(map.count, 0);

        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// make sure large counts don't overflow
    fn vi_count_overflow() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        // make sure large counts don't overflow our u32
        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
                KeyCode::Char('9'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// make sure large counts ending in zero don't overflow
    fn vi_count_overflow_zero() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        // make sure large counts don't overflow our u32
        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('1'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
                KeyCode::Char('0'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// Esc should cancel the count
    fn vi_count_cancel() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('1'),
                KeyCode::Char('0'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(map.count, 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test insert with a count
    fn vi_count_simple() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                //same as KeyCode::Esc
                Key::new_mod(KeyCode::Char('['), KeyMod::Ctrl),
                Key::new(KeyCode::Char('3')),
                Key::new(KeyCode::Char('i')),
                Key::new(KeyCode::Char('t')),
                Key::new(KeyCode::Char('h')),
                Key::new(KeyCode::Char('i')),
                Key::new(KeyCode::Char('s')),
                Key::new(KeyCode::Esc),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "thisthisthis");
    }

    #[test]
    /// test dot command
    fn vi_dot_command() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('f'),
                KeyCode::Esc,
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "iiifff");
    }

    #[test]
    /// test dot command with repeat
    fn vi_dot_command_repeat() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('f'),
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "iifififf");
    }

    #[test]
    /// test dot command with repeat
    fn vi_dot_command_repeat_multiple() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('f'),
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "iififiifififff");
    }

    #[test]
    /// test dot command with append
    fn vi_dot_command_append() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('a'),
                KeyCode::Char('i'),
                KeyCode::Char('f'),
                KeyCode::Esc,
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "ififif");
    }

    #[test]
    /// test dot command with append and repeat
    fn vi_dot_command_append_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('a'),
                KeyCode::Char('i'),
                KeyCode::Char('f'),
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "ifififif");
    }

    #[test]
    /// test dot command with movement
    fn vi_dot_command_movement() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('a'),
                KeyCode::Char('d'),
                KeyCode::Char('t'),
                KeyCode::Char(' '),
                KeyCode::Left,
                KeyCode::Left,
                KeyCode::Char('a'),
                KeyCode::Esc,
                KeyCode::Right,
                KeyCode::Right,
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "data ");
    }

    #[test]
    /// test move_count function
    fn move_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        assert_eq!(map.move_count(), 1);
        map.count = 1;
        assert_eq!(map.move_count(), 1);
        map.count = 99;
        assert_eq!(map.move_count(), 99);
    }

    #[test]
    /// make sure the count is reset if movement occurs
    fn vi_count_movement_reset() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('i'),
                KeyCode::Char('t'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Char('s'),
                KeyCode::Left,
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "this");
    }

    #[test]
    /// test movement with counts
    fn movement_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('3'), KeyCode::Left].iter(),
        );

        assert_eq!(ed.cursor(), 1);
    }

    #[test]
    /// test movement with counts, then insert (count should be reset before insert)
    fn movement_with_count_then_insert() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Left,
                KeyCode::Char('i'),
                KeyCode::Char(' '),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "r ight");
    }

    #[test]
    /// make sure we only attempt to repeat for as many chars are in the buffer
    fn count_at_buffer_edge() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
            ]
            .iter(),
        );
        // the cursor should not have moved and no change should have occured
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "replace");
    }

    #[test]
    /// test basic replace
    fn basic_replace() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('r'), KeyCode::Char('x')].iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "replacx");
    }

    #[test]
    fn replace_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('r'),
                KeyCode::Char(' '),
            ]
            .iter(),
        );
        // cursor should be on the last replaced char
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "   lace");
    }

    #[test]
    /// make sure replace won't work if there aren't enough chars
    fn replace_with_count_eol() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
            ]
            .iter(),
        );
        // the cursor should not have moved and no change should have occured
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "replace");
    }

    #[test]
    /// make sure normal mode is enabled after replace
    fn replace_then_normal() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('r'),
                KeyCode::Char('x'),
                KeyCode::Char('0'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "replacx");
    }

    #[test]
    /// test replace with dot
    fn dot_replace() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
                KeyCode::Char('7'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "xxxxxxx");
    }

    #[test]
    /// test replace with dot
    fn dot_replace_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "xxxxxxx");
    }

    #[test]
    /// test replace with dot at eol
    fn dot_replace_eol() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("test").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "xxxt");
    }

    #[test]
    /// test replace with dot at eol multiple times
    fn dot_replace_eol_multiple() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("this is a test").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('r'),
                KeyCode::Char('x'),
                KeyCode::Char('$'),
                KeyCode::Char('.'),
                KeyCode::Char('4'),
                KeyCode::Char('h'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "xxxs is axxxst");
    }

    #[test]
    /// verify our move count
    fn move_count_right() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);
        assert_eq!(map.move_count_right(&ed), 0);
        map.count = 10;
        assert_eq!(map.move_count_right(&ed), 0);

        map.count = 0;

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc, KeyCode::Left].iter());
        assert_eq!(map.move_count_right(&ed), 1);
    }

    #[test]
    /// verify our move count
    fn move_count_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);
        assert_eq!(map.move_count_left(&ed), 1);
        map.count = 10;
        assert_eq!(map.move_count_left(&ed), 7);

        map.count = 0;

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc, KeyCode::Char('0')].iter());
        assert_eq!(map.move_count_left(&ed), 0);
    }

    #[test]
    /// test delete with dot
    fn dot_x_delete() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace").unwrap();
        assert_eq!(ed.cursor(), 7);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('x'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "ace");
    }

    #[test]
    /// test deleting a line
    fn delete_line() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('d'), KeyCode::Char('d')].iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test for normal mode after deleting a line
    fn delete_line_normal() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('d'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('e'),
                KeyCode::Char('w'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "new");
    }

    #[test]
    /// test aborting a delete (and change)
    fn delete_abort() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("don't delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('d'),
                KeyCode::Esc,
                KeyCode::Char('d'),
                KeyCode::Char('c'),
                KeyCode::Char('c'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 11);
        assert_eq!(String::from(ed), "don't delete");
    }

    #[test]
    /// test deleting a single char to the left
    fn delete_char_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('d'), KeyCode::Char('h')].iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "delee");
    }

    #[test]
    /// test deleting multiple chars to the left
    fn delete_chars_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('d'),
                KeyCode::Char('h'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "dee");
    }

    #[test]
    /// test deleting a single char to the right
    fn delete_char_right() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('l'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "elete");
    }

    #[test]
    /// test deleting multiple chars to the right
    fn delete_chars_right() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('d'),
                KeyCode::Char('l'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "ete");
    }

    #[test]
    /// test repeat with delete
    fn delete_and_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('l'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "lete");
    }

    #[test]
    /// test delete until end of line
    fn delete_until_end() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('$'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test delete until end of line
    fn delete_until_end_shift_d() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('0'), KeyCode::Char('D')].iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test delete until start of line
    fn delete_until_start() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('$'),
                KeyCode::Char('d'),
                KeyCode::Char('0'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "e");
    }

    #[test]
    /// test a compound count with delete
    fn delete_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('2'),
                KeyCode::Char('l'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "te");
    }

    #[test]
    /// test a compound count with delete and repeat
    fn delete_with_count_and_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete delete").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('2'),
                KeyCode::Char('l'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "elete");
    }

    #[test]
    fn move_to_end_of_word_simple() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here are").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor(" som").unwrap();
        let end_pos = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();
        let vi = Vi::new();
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos);
    }

    #[test]
    fn move_to_end_of_word_comma() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_after_cursor('e').unwrap();
        let end_pos1 = ed.cursor();
        ed.insert_str_after_cursor(", som").unwrap();
        let end_pos2 = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos1);
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos2);
    }

    #[test]
    fn move_to_end_of_word_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("e,,,").unwrap();
        let end_pos1 = ed.cursor();
        ed.insert_str_after_cursor(",som").unwrap();
        let end_pos2 = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos1);
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos2);
    }

    #[test]
    fn move_to_end_of_word_whitespace() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        assert_eq!(ed.cursor(), 0);
        ed.insert_str_after_cursor("here are").unwrap();
        let start_pos = ed.cursor();
        assert_eq!(ed.cursor(), 8);
        ed.insert_str_after_cursor("      som").unwrap();
        assert_eq!(ed.cursor(), 17);
        ed.insert_str_after_cursor("e words").unwrap();
        assert_eq!(ed.cursor(), 24);
        ed.move_cursor_to(start_pos).unwrap();
        assert_eq!(ed.cursor(), 8);

        let vi = Vi::new();
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), 17);
    }

    #[test]
    fn move_to_end_of_word_whitespace_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("e   ,,,").unwrap();
        let end_pos1 = ed.cursor();
        ed.insert_str_after_cursor(", som").unwrap();
        let end_pos2 = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos1);
        vi.move_to_end_of_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos2);
    }

    #[test]
    fn move_to_end_of_word_ws_simple() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here are").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor(" som").unwrap();
        let end_pos = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();
        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos);
    }

    #[test]
    fn move_to_end_of_word_ws_comma() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_after_cursor('e').unwrap();
        let end_pos1 = ed.cursor();
        ed.insert_str_after_cursor(", som").unwrap();
        let end_pos2 = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos1);
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos2);
    }

    #[test]
    fn move_to_end_of_word_ws_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("e,,,,som").unwrap();
        let end_pos = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();
        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos);
    }

    #[test]
    fn move_to_end_of_word_ws_whitespace() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here are").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("      som").unwrap();
        let end_pos = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos);
    }

    #[test]
    fn move_to_end_of_word_ws_whitespace_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ar").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("e   ,,,").unwrap();
        let end_pos1 = ed.cursor();
        ed.insert_str_after_cursor(", som").unwrap();
        let end_pos2 = ed.cursor();
        ed.insert_str_after_cursor("e words").unwrap();
        ed.move_cursor_to(start_pos).unwrap();

        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos1);
        vi.move_to_end_of_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), end_pos2);
    }

    #[test]
    fn move_word_simple() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("are ").unwrap();
        let pos2 = ed.cursor();
        ed.insert_str_after_cursor("some words").unwrap();
        ed.move_cursor_to_start_of_line().unwrap();

        let vi = Vi::new();
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
    }

    #[test]
    fn move_word_whitespace() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("   ").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("word").unwrap();
        let pos2 = ed.cursor();
        ed.move_cursor_to_start_of_line().unwrap();

        let vi = Vi::new();
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
    }

    #[test]
    fn move_word_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("..=").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("word").unwrap();
        let pos2 = ed.cursor();
        ed.move_cursor_to_start_of_line().unwrap();

        let vi = Vi::new();
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
    }

    #[test]
    fn move_word_whitespace_nonkeywords() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("..=   ").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("..=").unwrap();
        let pos2 = ed.cursor();
        ed.insert_str_after_cursor("word").unwrap();
        let pos3 = ed.cursor();
        ed.move_cursor_to_start_of_line().unwrap();

        let vi = Vi::new();
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos3);
    }

    #[test]
    fn move_word_and_back() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("are ").unwrap();
        let pos2 = ed.cursor();
        ed.insert_str_after_cursor("some").unwrap();
        let pos3 = ed.cursor();
        ed.insert_str_after_cursor("..= ").unwrap();
        let pos4 = ed.cursor();
        ed.insert_str_after_cursor("words").unwrap();
        let pos5 = ed.cursor();

        // make sure move_word() and move_word_back() are reflections of eachother

        let vi = Vi::new();
        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos3);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos4);
        vi.move_word(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos5);

        vi.move_word_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos4);
        vi.move_word_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos3);
        vi.move_word_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), 0);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos4);
        vi.move_word_ws(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos5);

        vi.move_word_ws_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos4);
        vi.move_word_ws_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word_ws_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws_back(&mut ed, 1).unwrap();
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    fn move_word_and_back_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here ").unwrap();
        ed.insert_str_after_cursor("are ").unwrap();
        let pos1 = ed.cursor();
        ed.insert_str_after_cursor("some").unwrap();
        let pos2 = ed.cursor();
        ed.insert_str_after_cursor("..= ").unwrap();
        ed.insert_str_after_cursor("words").unwrap();
        let pos3 = ed.cursor();

        let vi = Vi::new();
        // make sure move_word() and move_word_back() are reflections of eachother
        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word(&mut ed, 3).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), pos3);

        vi.move_word_back(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), pos2);
        vi.move_word_back(&mut ed, 3).unwrap();
        assert_eq!(ed.cursor(), 0);

        ed.move_cursor_to_start_of_line().unwrap();
        vi.move_word_ws(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), pos3);

        vi.move_word_ws_back(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), pos1);
        vi.move_word_ws_back(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    fn move_to_end_of_word_ws_whitespace_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();

        ed.insert_str_after_cursor("here are").unwrap();
        let start_pos = ed.cursor();
        ed.insert_str_after_cursor("      som").unwrap();
        ed.insert_str_after_cursor("e word").unwrap();
        let end_pos = ed.cursor();
        ed.insert_str_after_cursor("s and some").unwrap();

        ed.move_cursor_to(start_pos).unwrap();
        let vi = Vi::new();
        vi.move_to_end_of_word_ws(&mut ed, 2).unwrap();
        assert_eq!(ed.cursor(), end_pos);
    }

    #[test]
    /// test delete word
    fn delete_word() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("delete some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('w'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "some words");
    }

    #[test]
    /// test changing a line
    fn change_line() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('c'),
                KeyCode::Char('c'),
                KeyCode::Char('d'),
                KeyCode::Char('o'),
                KeyCode::Char('n'),
                KeyCode::Char('e'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "done");
    }

    #[test]
    /// test deleting a single char to the left
    fn change_char_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('c'),
                KeyCode::Char('h'),
                KeyCode::Char('e'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "chanee");
    }

    #[test]
    /// test deleting multiple chars to the left
    fn change_chars_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('c'),
                KeyCode::Char('h'),
                KeyCode::Char('e'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "chee");
    }

    #[test]
    /// test deleting a single char to the right
    fn change_char_right() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('l'),
                KeyCode::Char('s'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "shange");
    }

    #[test]
    /// test changing multiple chars to the right
    fn change_chars_right() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('c'),
                KeyCode::Char('l'),
                KeyCode::Char('s'),
                KeyCode::Char('t'),
                KeyCode::Char('r'),
                KeyCode::Char('a'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "strange");
    }

    #[test]
    /// test repeat with change
    fn change_and_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('l'),
                KeyCode::Char('s'),
                KeyCode::Esc,
                KeyCode::Char('l'),
                KeyCode::Char('.'),
                KeyCode::Char('l'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "sssnge");
    }

    #[test]
    /// test change until end of line
    fn change_until_end() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('$'),
                KeyCode::Char('o'),
                KeyCode::Char('k'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "ok");
    }

    #[test]
    /// test change until end of line
    fn change_until_end_shift_c() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('C'),
                KeyCode::Char('o'),
                KeyCode::Char('k'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
        assert_eq!(String::from(ed), "ok");
    }

    #[test]
    /// test change until end of line
    fn change_until_end_from_middle_shift_c() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('l'),
                KeyCode::Char('C'),
                KeyCode::Char(' '),
                KeyCode::Char('o'),
                KeyCode::Char('k'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "ch ok");
    }

    #[test]
    /// test change until start of line
    fn change_until_start() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('$'),
                KeyCode::Char('c'),
                KeyCode::Char('0'),
                KeyCode::Char('s'),
                KeyCode::Char('t'),
                KeyCode::Char('r'),
                KeyCode::Char('a'),
                KeyCode::Char('n'),
                KeyCode::Char('g'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "strange");
    }

    #[test]
    /// test a compound count with change
    fn change_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('c'),
                KeyCode::Char('2'),
                KeyCode::Char('l'),
                KeyCode::Char('s'),
                KeyCode::Char('t'),
                KeyCode::Char('r'),
                KeyCode::Char('a'),
                KeyCode::Char('n'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "strange");
    }

    #[test]
    /// test a compound count with change and repeat
    fn change_with_count_and_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change change").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('c'),
                KeyCode::Char('2'),
                KeyCode::Char('l'),
                KeyCode::Char('o'),
                KeyCode::Esc,
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "ochange");
    }

    #[test]
    /// test change word
    fn change_word() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('w'),
                KeyCode::Char('t'),
                KeyCode::Char('w'),
                KeyCode::Char('e'),
                KeyCode::Char('a'),
                KeyCode::Char('k'),
                KeyCode::Char(' '),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "tweak some words");
    }

    #[test]
    /// make sure the count is properly reset
    fn test_count_reset_around_insert_and_delete() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("these are some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('3'),
                KeyCode::Char('w'),
                KeyCode::Char('i'),
                KeyCode::Char('w'),
                KeyCode::Char('o'),
                KeyCode::Char('r'),
                KeyCode::Char('d'),
                KeyCode::Char('s'),
                KeyCode::Char(' '),
                KeyCode::Esc,
                KeyCode::Char('l'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "words words words");
    }

    #[test]
    /// make sure t command does nothing if nothing was found
    fn test_t_not_found() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('t'),
                KeyCode::Char('z'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
    }

    #[test]
    /// make sure t command moves the cursor
    fn test_t_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('t'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
    }

    #[test]
    /// make sure t command moves the cursor
    fn test_t_movement_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg d").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('t'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 8);
    }

    #[test]
    /// test normal mode after char movement
    fn test_t_movement_then_normal() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('t'),
                KeyCode::Char('d'),
                KeyCode::Char('l'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    /// test delete with char movement
    fn test_t_movement_with_delete() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('t'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "defg");
    }

    #[test]
    /// test change with char movement
    fn test_t_movement_with_change() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('c'),
                KeyCode::Char('t'),
                KeyCode::Char('d'),
                KeyCode::Char('z'),
                KeyCode::Char(' '),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "z defg");
    }

    #[test]
    /// make sure f command moves the cursor
    fn test_f_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('f'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    /// make sure T command moves the cursor
    fn test_cap_t_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('$'),
                KeyCode::Char('T'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
    }

    #[test]
    /// make sure F command moves the cursor
    fn test_cap_f_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('$'),
                KeyCode::Char('F'),
                KeyCode::Char('d'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    /// make sure ; command moves the cursor
    fn test_semi_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc abc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('f'),
                KeyCode::Char('c'),
                KeyCode::Char(';'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
    }

    #[test]
    /// make sure , command moves the cursor
    fn test_comma_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc abc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('f'),
                KeyCode::Char('c'),
                KeyCode::Char('$'),
                KeyCode::Char(','),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 2);
    }

    #[test]
    /// test delete with semi (;)
    fn test_semi_delete() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc abc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('f'),
                KeyCode::Char('c'),
                KeyCode::Char('d'),
                KeyCode::Char(';'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "ab");
    }

    #[test]
    /// test delete with semi (;) and repeat
    fn test_semi_delete_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc abc abc abc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('f'),
                KeyCode::Char('c'),
                KeyCode::Char('d'),
                KeyCode::Char(';'),
                KeyCode::Char('.'),
                KeyCode::Char('.'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "ab");
    }

    #[test]
    /// test find_char
    fn test_find_char() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcdefg").unwrap();
        assert_eq!(super::find_char(ed.current_buffer(), 0, 'd', 1), Some(3));
    }

    #[test]
    /// test find_char with non-zero start
    fn test_find_char_with_start() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcabc").unwrap();
        assert_eq!(super::find_char(ed.current_buffer(), 1, 'a', 1), Some(3));
    }

    #[test]
    /// test find_char with unicode symbol
    fn test_find_char_unicode() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abc\u{938}\u{94d}\u{924}\u{947}abc")
            .unwrap();
        assert_eq!(super::find_char(ed.current_buffer(), 0, 'a', 2), Some(5));
    }

    #[test]
    /// test find_char with count
    fn test_find_char_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcabc").unwrap();
        assert_eq!(super::find_char(ed.current_buffer(), 0, 'a', 2), Some(3));
    }

    #[test]
    /// test find_char not found
    fn test_find_char_not_found() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcdefg").unwrap();
        assert_eq!(super::find_char(ed.current_buffer(), 0, 'z', 1), None);
    }

    #[test]
    /// test find_char_rev
    fn test_find_char_rev() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcdefg").unwrap();
        assert_eq!(
            super::find_char_rev(ed.current_buffer(), 6, 'd', 1),
            Some(3)
        );
    }

    #[test]
    /// test find_char_rev with non-zero start
    fn test_find_char_rev_with_start() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcabc").unwrap();
        assert_eq!(
            super::find_char_rev(ed.current_buffer(), 5, 'c', 1),
            Some(2)
        );
    }

    #[test]
    /// test find_char_rev with unicode symbol
    fn test_find_char_rev_unicode() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abc\u{938}\u{94d}\u{924}\u{947}abc")
            .unwrap();
        assert_eq!(
            super::find_char_rev(ed.current_buffer(), 5, 'a', 1),
            Some(0)
        );
    }

    #[test]
    /// test find_char_rev with count
    fn test_find_char_rev_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcabc").unwrap();
        assert_eq!(
            super::find_char_rev(ed.current_buffer(), 6, 'c', 2),
            Some(2)
        );
    }

    #[test]
    /// test find_char_rev not found
    fn test_find_char_rev_not_found() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        ed.insert_str_after_cursor("abcdefg").unwrap();
        assert_eq!(super::find_char_rev(ed.current_buffer(), 6, 'z', 1), None);
    }

    #[test]
    /// undo with counts
    fn test_undo_with_counts() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abcdefg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('x'),
                KeyCode::Char('x'),
                KeyCode::Char('x'),
                KeyCode::Char('3'),
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "abcdefg");
    }

    #[test]
    /// redo with counts
    fn test_redo_with_counts() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abcdefg").unwrap();

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new(KeyCode::Esc),
                Key::new(KeyCode::Char('x')),
                Key::new(KeyCode::Char('x')),
                Key::new(KeyCode::Char('x')),
                Key::new(KeyCode::Char('u')),
                Key::new(KeyCode::Char('u')),
                Key::new(KeyCode::Char('u')),
            ]
            .iter(),
        );
        // Ctrl-r taken by incremental search so do this manually.
        map.handle_redo(&mut ed).unwrap();
        map.handle_redo(&mut ed).unwrap();
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "abcde");
    }

    #[test]
    /// test change word with 'gE'
    fn change_word_ge_ws() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("change some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('c'),
                KeyCode::Char('g'),
                KeyCode::Char('E'),
                KeyCode::Char('e'),
                KeyCode::Char('t'),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('g'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "change something");
    }

    #[test]
    /// test undo in groups
    fn undo_insert() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test undo in groups
    fn undo_insert2() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('i'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test undo in groups
    fn undo_insert_with_history() {
        let mut out = Vec::new();
        let mut history = History::new();
        history.push(Buffer::from("insert-xxx")).unwrap();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('i'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Up,
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Char('s'),
                KeyCode::Char('t'),
                KeyCode::Char('o'),
                KeyCode::Char('r'),
                KeyCode::Char('y'),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Char('t'),
                KeyCode::Char('e'),
                KeyCode::Char('x'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "insert");
    }

    #[test]
    /// test undo in groups
    fn undo_insert_with_history2() {
        let mut history = History::new();
        history.push(Buffer::from("")).unwrap();
        let mut out = Vec::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('i'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Up,
                KeyCode::Esc,
                KeyCode::Down,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    /// test undo in groups
    fn undo_insert_with_movement_reset() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('i'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                // movement reset will get triggered here
                KeyCode::Left,
                KeyCode::Right,
                KeyCode::Char(' '),
                KeyCode::Char('t'),
                KeyCode::Char('e'),
                KeyCode::Char('x'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "insert");
    }

    #[test]
    /// test undo in groups
    fn undo_3x() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("rm some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('x'),
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "rm some words");
    }

    #[test]
    /// test undo in groups
    fn undo_insert_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('i'),
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "insert");
    }

    #[test]
    /// test undo in groups
    fn undo_insert_with_repeat() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Char('i'),
                KeyCode::Char('n'),
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('t'),
                KeyCode::Esc,
                KeyCode::Char('3'),
                KeyCode::Char('.'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "insert");
    }

    #[test]
    /// test undo in groups
    fn undo_s_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor(
            "replace so\u{1f469}\u{200d}\u{1f4bb}\u{1f469}\u{200d}\u{1f4bb}me words",
        )
        .unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('8'),
                KeyCode::Char('s'),
                KeyCode::Char('o'),
                KeyCode::Char('k'),
                KeyCode::Esc,
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(
            String::from(ed),
            "replace so\u{1f469}\u{200d}\u{1f4bb}\u{1f469}\u{200d}\u{1f4bb}me words"
        );
    }

    #[test]
    /// test undo in groups
    fn undo_multiple_groups() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('A'),
                KeyCode::Char(' '),
                KeyCode::Char('h'),
                KeyCode::Char('e'),
                KeyCode::Char('r'),
                KeyCode::Char('e'),
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('8'),
                KeyCode::Char('s'),
                KeyCode::Char('o'),
                KeyCode::Char('k'),
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "replace some words");
    }

    #[test]
    /// test undo in groups
    fn undo_r_command_with_count() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("replace some words").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('8'),
                KeyCode::Char('r'),
                KeyCode::Char(' '),
                KeyCode::Char('u'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "replace some words");
    }

    #[test]
    /// test tilde
    fn tilde_basic() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("tilde").unwrap();

        simulate_key_codes(&mut map, &mut ed, [KeyCode::Esc, KeyCode::Char('~')].iter());
        assert_eq!(String::from(ed), "tildE");
    }

    #[test]
    /// test tilde
    fn tilde_basic2() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("tilde").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('~'), KeyCode::Char('~')].iter(),
        );
        assert_eq!(String::from(ed), "tilde");
    }

    #[test]
    /// test tilde
    fn tilde_move() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("tilde").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('~'),
                KeyCode::Char('~'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "TIlde");
    }

    #[test]
    /// test tilde
    fn tilde_repeat() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("tilde").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('~'), KeyCode::Char('.')].iter(),
        );
        assert_eq!(String::from(ed), "tilde");
    }

    #[test]
    /// test tilde
    fn tilde_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("tilde").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('1'),
                KeyCode::Char('0'),
                KeyCode::Char('~'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "TILDE");
    }

    #[test]
    /// test tilde
    fn tilde_count_short() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("TILDE").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('~'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "tiLDE");
    }

    #[test]
    /// test tilde
    fn tilde_nocase() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("ti_lde").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('6'),
                KeyCode::Char('~'),
            ]
            .iter(),
        );
        assert_eq!(String::from(ed), "TI_LDE");
    }

    #[test]
    /// ctrl-h should act as backspace
    fn ctrl_h() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("not empty").unwrap();

        let res = map.handle_key(
            Key::new_mod(KeyCode::Char('h'), KeyMod::Ctrl),
            &mut ed,
            &mut EmptyCompleter,
        );
        assert_eq!(res.is_ok(), true);
        assert_eq!(ed.current_buffer().to_string(), "not empt".to_string());
    }

    #[test]
    /// repeat char move with no last char
    fn repeat_char_move_no_char() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Esc, KeyCode::Char('$'), KeyCode::Char(';')].iter(),
        );
        assert_eq!(ed.cursor(), 7);
    }

    #[test]
    fn test_yank_and_put_back() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("abc defg").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('y'),
                KeyCode::Char('$'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 7);
        assert_eq!(String::from(ed), "abc defgabc defg");
    }

    #[test]
    fn test_delete_surround_text_object() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("(ab\u{1f469}\u{200d}\u{1f4bb} \u{1f469}\u{200d}\u{1f4bb}efg)")
            .unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('('),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "()");
    }

    #[test]
    fn test_delete_surround_empty_text_object() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("()").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('('),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "()");
    }

    #[test]
    fn test_delete_surround_text_object_paste() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("aaaa(Bbbb)cccc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('2'),
                KeyCode::Char('w'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char(')'),
                KeyCode::Char('p'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 9);
        assert_eq!(String::from(ed), "aaaa()Bbbbcccc");
    }

    #[test]
    fn test_delete_surround_text_object_over_match_character_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("`aaaa` bbbb `cccc`").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('5'),
                KeyCode::Char('h'),
                KeyCode::Char('d'),
                KeyCode::Char('a'),
                KeyCode::Char('`'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 5);
        assert_eq!(String::from(ed), "`aaaacccc`");
    }

    #[test]
    fn test_delete_surround_text_object_over_match_character_no_match() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("aaaa bbbb 'cccc").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('4'),
                KeyCode::Char('h'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('\''),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 10);
        assert_eq!(String::from(ed), "aaaa bbbb 'cccc");
    }

    #[test]
    fn test_yank_surround_text_object() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("echo \"hello world\u{1f469}\u{200d}\u{1f52c}\"")
            .unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('y'),
                KeyCode::Char('i'),
                KeyCode::Char('"'),
                KeyCode::Char('P'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 17);
        assert_eq!(
            String::from(ed),
            "echo \"hello world\u{1f469}\u{200d}\u{1f52c}hello world\u{1f469}\u{200d}\u{1f52c}\""
        );
    }

    #[test]
    fn test_change_surround_text_object() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("echo ").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('c'),
                KeyCode::Char('a'),
                KeyCode::Char('}'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "echo ");
    }

    #[test]
    fn test_change_and_insert_surround_text_object() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor(
            "echo [hello \u{1f469}\u{200d}\u{1f52c}\u{1f469}\u{200d}\u{1f52c} world]",
        )
        .unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('c'),
                KeyCode::Char('i'),
                KeyCode::Char('['),
                KeyCode::Char('o'),
                KeyCode::Char('h'),
                KeyCode::Char(' '),
                KeyCode::Char('h'),
                KeyCode::Char('i'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 10);
        assert_eq!(String::from(ed), "echo [oh hi]");
    }

    #[test]
    fn test_yank_and_delete_surround_xml() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor(
            "<div id='foo'>\u{1f468}\u{200d}\u{1f52c}content\u{1f468}\u{200d}\u{1f52c}</p>",
        )
        .unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('w'),
                KeyCode::Char('c'),
                KeyCode::Char('i'),
                KeyCode::Char('>'),
                KeyCode::Char('p'),
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('w'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('t'),
                KeyCode::Esc,
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "<p></p>");
    }

    #[test]
    fn test_do_not_match_asymmetrical_surround_objects() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("aaabbb)ccc)ddd").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('4'),
                KeyCode::Char('b'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('('),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "aaabbb)ccc)ddd");
    }

    #[test]
    fn test_do_not_match_unbalanced_asymmetrical_surround_objects_close() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("aaaa(bbbb)ccc)ddd").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('4'),
                KeyCode::Char('h'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char('('),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 12);
        assert_eq!(String::from(ed), "aaaa(bbbb)ccc)ddd");
    }

    #[test]
    fn test_do_not_match_unbalanced_asymmetrical_surround_objects_open() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("(aaaa(bbbb)cccddd").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char(')'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "(aaaa(bbbb)cccddd");
    }

    #[test]
    fn test_do_not_match_surround_balanced_text_objects_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("(aaaa(bbbb)cccddd").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('w'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char(')'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 6);
        assert_eq!(String::from(ed), "(aaaa(bbbb)cccddd");
    }

    #[test]
    fn test_match_surround_balanced_text_objects_with_count() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let rules = DefaultEditorRules::default();
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            &rules,
        )
        .unwrap();
        let mut map = Vi::new();
        map.init(&mut ed);
        ed.insert_str_after_cursor("(aaaa(bbbb)cccddd)").unwrap();

        simulate_key_codes(
            &mut map,
            &mut ed,
            [
                KeyCode::Esc,
                KeyCode::Char('0'),
                KeyCode::Char('3'),
                KeyCode::Char('w'),
                KeyCode::Char('2'),
                KeyCode::Char('d'),
                KeyCode::Char('i'),
                KeyCode::Char(')'),
            ]
            .iter(),
        );
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "()");
    }
}
