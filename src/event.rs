use crate::Editor;
use sl_console::event::Key;

pub struct Event<'a, 'out: 'a> {
    pub editor: &'a mut Editor<'out>,
    pub kind: EventKind,
}

impl<'a, 'out: 'a> Event<'a, 'out> {
    pub fn new(editor: &'a mut Editor<'out>, kind: EventKind) -> Self {
        Event { editor, kind }
    }
}

#[derive(Debug)]
pub enum EventKind {
    /// Sent before handling a keypress.
    BeforeKey(Key),
    /// Sent after handling a keypress.
    AfterKey(Key),
    /// Sent in `Editor.complete()`, before processing the completion.
    BeforeComplete,
}
