use crate::Buffer;

pub trait NewlineRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf)
    }
}

pub trait WordDivideRule {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        get_buffer_words(buf)
    }
}

pub trait EditorRules
where
    Self: WordDivideRule + NewlineRule,
{
}

pub struct NewlineDefaultRule;
impl NewlineRule for NewlineDefaultRule {}

pub struct WordDividerDefaultRule;
impl WordDivideRule for WordDividerDefaultRule {}

pub struct NewlineForBackslashAndOpenDelimRule;

impl NewlineRule for NewlineForBackslashAndOpenDelimRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf) && check_balanced_delimiters(buf)
    }
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

pub fn get_buffer_words(buf: &Buffer) -> Vec<(usize, usize)> {
    let mut res = Vec::new();

    let mut word_start = None;
    let mut just_had_backslash = false;

    let buf_vec = buf.range_graphemes_all();
    for (i, c) in buf_vec.enumerate() {
        if c == "\\" {
            //TODO interaction with NewlineRule?
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
