extern crate liner;
extern crate regex;
extern crate termion;

use std::env::{args, current_dir};
use std::io;
use std::mem::replace;

use liner::keymap;
use liner::{Completer, Context, CursorPosition, Event, EventKind, FilenameCompleter};
use regex::Regex;
use termion::color;

fn highlight_dodo(s: &str) -> String {
    let reg_exp = Regex::new("(?P<k>dodo)").unwrap();
    let format = format!("{}$k{}", color::Fg(color::Red), color::Fg(color::Reset));
    reg_exp.replace_all(s, format.as_str()).to_string()
}

struct NoCommentCompleter {
    inner: Option<FilenameCompleter>,
}

impl Completer for NoCommentCompleter {
    fn completions(&mut self, start: &str) -> Vec<String> {
        if let Some(inner) = &mut self.inner {
            inner.completions(start)
        } else {
            Vec::new()
        }
    }

    fn on_event(&mut self, event: Event) {
        if let EventKind::BeforeComplete = event.kind {
            let (_, pos) = event.editor.get_words_and_cursor_position();

            // Figure out of we are completing a command (the first word) or a filename.
            let filename = match pos {
                CursorPosition::InWord(i) => i > 0,
                CursorPosition::InSpace(Some(_), _) => true,
                CursorPosition::InSpace(None, _) => false,
                CursorPosition::OnWordLeftEdge(i) => i >= 1,
                CursorPosition::OnWordRightEdge(i) => i >= 1,
            };

            if filename {
                let completer = FilenameCompleter::new(Some(current_dir().unwrap()));
                replace(&mut self.inner, Some(completer));
            } else {
                replace(&mut self.inner, None);
            }
        }
    }
}

fn main() {
    let mut con = Context::new();
    let mut completer = NoCommentCompleter { inner: None };

    let history_file = match args().nth(1) {
        Some(file_name) => {
            println!("History file: {}", file_name);
            file_name
        }
        None => {
            eprintln!("No history file provided. Ending example early.");
            return;
        }
    };

    con.history
        .set_file_name_and_load_history(history_file)
        .unwrap();

    let mut keymap: Box<dyn keymap::KeyMap> = Box::new(keymap::Emacs::new());

    loop {
        let res = con.read_line(
            "[prompt]$ ",
            Some(Box::new(highlight_dodo)),
            &mut completer,
            Some(&mut *keymap),
        );

        match res {
            Ok(res) => {
                match res.as_str() {
                    "emacs" => {
                        keymap = Box::new(keymap::Emacs::new());
                        println!("emacs mode");
                    }
                    "vi" => {
                        keymap = Box::new(keymap::Vi::new());
                        println!("vi mode");
                    }
                    "exit" | "" => {
                        println!("exiting...");
                        break;
                    }
                    _ => {}
                }

                if res.is_empty() {
                    break;
                }

                con.history.push(res.into()).unwrap();
            }
            Err(e) => {
                match e.kind() {
                    // ctrl-c pressed
                    io::ErrorKind::Interrupted => {}
                    // ctrl-d pressed
                    io::ErrorKind::UnexpectedEof => {
                        println!("exiting...");
                        break;
                    }
                    _ => {
                        // Ensure that all writes to the history file
                        // are written before exiting.
                        panic!("error: {:?}", e)
                    }
                }
            }
        }
    }
    // Ensure that all writes to the history file are written before exiting.
    con.history.commit_to_file();
}
