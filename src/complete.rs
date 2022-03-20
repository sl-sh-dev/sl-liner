use super::event::Event;
use std::path::PathBuf;
use crate::EditorRules;

pub trait Completer {
    fn completions(&mut self, start: &str) -> Vec<String>;
    fn on_event<T: EditorRules>(&mut self, _event: Event<T>) {}
}

/// Completer with no completions
pub struct EmptyCompleter {
    empty: Vec<String>,
}

impl EmptyCompleter {
    pub fn new() -> EmptyCompleter {
        EmptyCompleter {
            empty: Vec::with_capacity(0),
        }
    }
}

impl Default for EmptyCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for EmptyCompleter {
    fn completions(&mut self, _start: &str) -> Vec<String> {
        self.empty.clone()
    }
}

/// Completer that can be seeded with a list of prefixes..
pub struct BasicCompleter {
    prefixes: Vec<String>,
}

impl BasicCompleter {
    pub fn new<T: Into<String>>(prefixes: Vec<T>) -> BasicCompleter {
        BasicCompleter {
            prefixes: prefixes.into_iter().map(|s| s.into()).collect(),
        }
    }
}

impl Completer for BasicCompleter {
    fn completions(&mut self, start: &str) -> Vec<String> {
        self.prefixes
            .iter()
            .filter(|s| s.starts_with(start))
            .cloned()
            .collect()
    }
}

/// Completer for filenames in the current working_dir
pub struct FilenameCompleter {
    working_dir: Option<PathBuf>,
    case_sensitive: bool,
}

impl FilenameCompleter {
    pub fn new<T: Into<PathBuf>>(working_dir: Option<T>) -> Self {
        FilenameCompleter {
            working_dir: working_dir.map(|p| p.into()),
            case_sensitive: true,
        }
    }

    pub fn with_case_sensitivity<T: Into<PathBuf>>(
        working_dir: Option<T>,
        case_sensitive: bool,
    ) -> Self {
        FilenameCompleter {
            working_dir: working_dir.map(|p| p.into()),
            case_sensitive,
        }
    }
}

impl Completer for FilenameCompleter {
    fn completions(&mut self, mut start: &str) -> Vec<String> {
        // XXX: this function is really bad, TODO rewrite

        let start_owned: String = if start.starts_with('\"') || start.starts_with('\'') {
            start = &start[1..];
            if !start.is_empty() {
                start = &start[..start.len() - 1];
            }
            start.into()
        } else {
            start.replace(r"\ ", " ")
        };

        let start_path = PathBuf::from(start_owned.as_str());

        let full_path = match self.working_dir {
            Some(ref wd) => {
                let mut fp = PathBuf::from(wd);
                fp.push(start_owned.as_str());
                fp
            }
            None => PathBuf::from(start_owned.as_str()),
        };

        let p;
        let mut start_name = None;
        let completing_dir;
        match full_path.parent() {
            // XXX non-unix separator
            Some(parent)
                if !start.is_empty()
                    && !start_owned.ends_with('/')
                    && !full_path.ends_with("..") =>
            {
                p = parent;
                if let Some(file_name) = full_path.file_name() {
                    let sn = file_name.to_string_lossy();
                    start_name = {
                        if !self.case_sensitive {
                            let _ = sn.to_lowercase();
                        };
                        Some(sn)
                    }
                }
                completing_dir = false;
            }
            _ => {
                p = full_path.as_path();
                start_name = Some("".into());
                completing_dir =
                    start.is_empty() || start.ends_with('/') || full_path.ends_with("..");
            }
        }

        let read_dir = match p.read_dir() {
            Ok(x) => x,
            Err(_) => return vec![],
        };

        let mut matches = vec![];
        for dir in read_dir {
            let dir = match dir {
                Ok(x) => x,
                Err(_) => continue,
            };
            let file_name = dir.file_name();
            let file_name = if self.case_sensitive {
                file_name.to_string_lossy().to_string()
            } else {
                file_name.to_string_lossy().to_lowercase()
            };

            if let Some(start_name) = &start_name {
                if file_name.starts_with(&**start_name) {
                    let mut a = start_path.clone();
                    if !a.is_absolute() {
                        a = PathBuf::new();
                    } else if !completing_dir && !a.pop() {
                        return vec![];
                    }

                    a.push(dir.file_name());
                    let mut s = a.to_string_lossy();
                    if dir.path().is_dir() {
                        let mut string = s.into_owned();
                        string.push('/');
                        s = string.into();
                    }

                    let mut b = PathBuf::from(&start_owned);
                    if !completing_dir {
                        b.pop();
                    }
                    b.push(s.as_ref());

                    matches.push(b.to_string_lossy().replace(' ', r"\ "));
                }
            }
        }

        matches
    }
}
