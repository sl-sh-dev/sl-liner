//! A readline-like library
//! For more information refer to the [README](https://github.com/sl-sh-dev/sl-liner)
//! as well as the examples/ directory for a demonstration of how to create and customize a Context.
extern crate bytecount;
extern crate sl_console;
extern crate unicode_width;

mod event;
pub use event::*;

mod editor;
pub use editor::*;

mod complete;
pub use complete::*;

mod context;
pub use context::*;

mod buffer;
pub use buffer::*;

mod terminal;
use terminal::*;

mod history;
pub use history::*;

pub mod keymap;
pub use keymap::*;

pub mod prompt;
pub use prompt::*;

pub mod cursor;
mod util;
pub use cursor::*;

pub mod editor_rules;
pub use editor_rules::*;

mod grapheme_iter;
#[cfg(test)]
mod test;
