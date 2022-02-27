use crate::get_buffer_words;
use crate::Buffer;

pub struct NewlineDefaultRule;

pub trait NewlineRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool;
}

pub trait WordDivideRule {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)>;
}

pub trait EditorRules
where
    Self: WordDivideRule + NewlineRule,
{
}

impl NewlineRule for NewlineDefaultRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf)
    }
}

pub struct NewlineForBackslashAndOpenDelimRule;

impl NewlineRule for NewlineForBackslashAndOpenDelimRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf) && check_balanced_delimiters(buf)
    }
}

pub struct WordDividerDefaultRule;

impl WordDivideRule for WordDividerDefaultRule {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        get_buffer_words(buf)
    }
}

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

pub struct EditorRulesBuilder {
    word_divider_fn: Option<Box<dyn WordDivideRule>>,
    line_completion_fn: Option<Box<dyn NewlineRule>>,
}

impl Default for EditorRulesBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorRulesBuilder {
    pub fn new() -> Self {
        EditorRulesBuilder {
            word_divider_fn: None,
            line_completion_fn: None,
        }
    }

    pub fn set_word_divider_fn(mut self, word_divider_fn: Box<dyn WordDivideRule>) -> Self {
        self.word_divider_fn = Some(word_divider_fn);
        self
    }

    pub fn set_line_completion_fn(mut self, line_completion_fn: Box<dyn NewlineRule>) -> Self {
        self.line_completion_fn = Some(line_completion_fn);
        self
    }

    pub fn build(self) -> ContextHelper {
        ContextHelper {
            word_divider_fn: self
                .word_divider_fn
                .unwrap_or_else(|| Box::new(WordDividerDefaultRule {})),
            line_completion_fn: self
                .line_completion_fn
                .unwrap_or_else(|| Box::new(NewlineDefaultRule {})),
        }
    }
}

pub struct ContextHelper {
    word_divider_fn: Box<dyn WordDivideRule>,
    line_completion_fn: Box<dyn NewlineRule>,
}

impl EditorRules for ContextHelper {}

impl NewlineRule for ContextHelper {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        self.line_completion_fn.evaluate_on_newline(buf)
    }
}

impl WordDivideRule for ContextHelper {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        self.word_divider_fn.divide_words(buf)
    }
}

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
