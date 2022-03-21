//! User-defined prompt.
use std::fmt;

/// User-defined prompt.
///
/// # Examples
///
/// You simply define a static prompt that holds a string.
/// The prefix and suffix fields are intended for keybinds to change the
/// prompt (ie the mode in vi).
/// ```
/// # use sl_liner::Prompt;
/// let prompt = Prompt::from("prompt$ ");
/// assert_eq!(&prompt.to_string(), "prompt$ ");
/// ```
pub struct Prompt {
    pub prefix: Option<String>,
    pub prompt: String,
    pub suffix: Option<String>,
}

impl Prompt {
    /// Constructs a static prompt.
    pub fn from<P: Into<String>>(prompt: P) -> Self {
        Prompt {
            prefix: None,
            prompt: prompt.into(),
            suffix: None,
        }
    }

    pub fn prefix(&self) -> &str {
        match &self.prefix {
            Some(prefix) => prefix,
            None => "",
        }
    }

    pub fn suffix(&self) -> &str {
        match &self.suffix {
            Some(suffix) => suffix,
            None => "",
        }
    }
}

impl fmt::Display for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.prefix(), self.prompt, self.suffix())
    }
}
