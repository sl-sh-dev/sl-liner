use crate::Buffer;

pub trait NewlineRule {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        last_non_ws_char_was_not_backslash(buf)
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

pub trait WordDivideRule {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        divide_words_by_space(buf)
    }
}

pub fn divide_words_by_space(buf: &Buffer) -> Vec<(usize, usize)> {
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


pub trait EditorRules
where
    Self: WordDivideRule + NewlineRule,
{
}

pub struct EditorRulesBuilder {
    word_divider_rule: Option<Box<dyn WordDivideRule>>,
    new_line_rule: Option<Box<dyn NewlineRule>>,
}

impl Default for EditorRulesBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorRulesBuilder {
    pub fn new() -> Self {
        EditorRulesBuilder {
            word_divider_rule: None,
            new_line_rule: None,
        }
    }

    pub fn set_word_divider_rule(mut self, word_divider_rule: Box<dyn WordDivideRule>) -> Self {
        self.word_divider_rule = Some(word_divider_rule);
        self
    }

    pub fn set_new_line_rule(mut self, newline_rule: Box<dyn NewlineRule>) -> Self {
        self.new_line_rule = Some(newline_rule);
        self
    }

    pub fn build(self) -> DefaultEditorRules {
        DefaultEditorRules {
            word_divider_rule: self
                .word_divider_rule
                .unwrap_or_else(|| Box::new(DefaultWordDividerRule {})),
            newline_rule: self
                .new_line_rule
                .unwrap_or_else(|| Box::new(DefaultNewlineRule {})),
        }
    }
}

pub struct DefaultEditorRules {
    word_divider_rule: Box<dyn WordDivideRule>,
    newline_rule: Box<dyn NewlineRule>,
}

impl EditorRules for DefaultEditorRules {}

pub struct DefaultNewlineRule;
impl NewlineRule for DefaultNewlineRule {}

pub struct DefaultWordDividerRule;
impl WordDivideRule for DefaultWordDividerRule {}

impl NewlineRule for DefaultEditorRules {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        self.newline_rule.evaluate_on_newline(buf)
    }
}

impl WordDivideRule for DefaultEditorRules {
    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        self.word_divider_rule.divide_words(buf)
    }
}