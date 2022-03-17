//! Provides [DefaultEditorRules] for editor behaviors with custom override function.
//! See [NewlineRule] and [WordDivideRule], the traits that compose editor behaviors.
use crate::Buffer;

/// When the client inputs the newline character, '\n', the Editor can decide to *not* evalute the
/// newline if the provided implementation of this trait returns false. Default prevents evaluate
/// if the last non whitespace character is a backslash.
pub trait NewlineRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf)
    }
}

/// Default NewlineRule implementation.
pub fn last_non_ws_char_was_not_backslash(buf: &Buffer) -> bool {
    let mut found_backslash = false;
    for x in buf.range_graphemes_all().rev() {
        if x == " " {
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

/// When the Editor is trying to place the term cursor it needs to know how to divide the words to
/// determine its [CursorPosition] state.
pub trait WordDivideRule {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        divide_words_by_space(buf)
    }
}

/// Default WordDivideRule implementation.
pub fn divide_words_by_space(buf: &Buffer) -> Vec<(usize, usize)> {
    let mut res = Vec::new();

    let mut word_start = None;
    let mut just_had_backslash = false;

    let buf_vec = buf.range_graphemes_all();
    for (i, c) in buf_vec.enumerate() {
        if c == "\\" {
            just_had_backslash = true;
            continue;
        }

        if let Some(start) = word_start {
            if c == " " && !just_had_backslash {
                res.push((start, i));
                word_start = None;
            }
        } else if c != " " {
            word_start = Some(i);
        }

        just_had_backslash = false;
    }

    if let Some(start) = word_start {
        res.push((start, buf.num_graphemes()));
    }

    res
}

/// Trait that implements all editor rule behaviors, a mechanism to allow modification of the
/// library's behavior at runtime.
pub trait EditorRules
where
    Self: WordDivideRule + NewlineRule,
{
}

/// Provides default editor behavior and provides custom override function.
pub struct DefaultEditorRules<T, U>
where
    T: WordDivideRule,
    U: NewlineRule,
{
    word_divider_rule: T,
    newline_rule: U,
}

impl Default for DefaultEditorRules<DefaultWordDivideRule, DefaultNewlineRule> {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultEditorRules<DefaultWordDivideRule, DefaultNewlineRule> {
    pub fn new() -> Self {
        DefaultEditorRules {
            word_divider_rule: DefaultWordDivideRule {},
            newline_rule: DefaultNewlineRule {},
        }
    }
}

impl<T, U> EditorRules for DefaultEditorRules<T, U>
where
    T: WordDivideRule,
    U: NewlineRule,
{
}

impl<T, U> DefaultEditorRules<T, U>
where
    T: WordDivideRule,
    U: NewlineRule,
{
    pub fn custom(t: T, u: U) -> Self {
        DefaultEditorRules {
            word_divider_rule: t,
            newline_rule: u,
        }
    }
}

/// Struct with default newline behavior
pub struct DefaultNewlineRule;
impl NewlineRule for DefaultNewlineRule {}

/// Struct with default word divide behavior
pub struct DefaultWordDivideRule;
impl WordDivideRule for DefaultWordDivideRule {}

impl<T, U> NewlineRule for DefaultEditorRules<T, U>
where
    T: WordDivideRule,
    U: NewlineRule,
{
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        self.newline_rule.evaluate_on_newline(buf)
    }
}

impl<T, U> WordDivideRule for DefaultEditorRules<T, U>
where
    T: WordDivideRule,
    U: NewlineRule,
{
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        self.word_divider_rule.divide_words(buf)
    }
}
