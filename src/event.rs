use crate::{Editor, EditorRules};
use sl_console::event::Key;

/// Event has context about the state of the editor and the EventKind and is consumed by Completer
pub struct Event<'a, 'out: 'a, T: EditorRules> {
    pub editor: &'a mut Editor<'out, T>,
    pub kind: EventKind,
}

impl<'a, 'out: 'a, T: EditorRules> Event<'a, 'out, T> {
    pub fn new(editor: &'a mut Editor<'out, T>, kind: EventKind) -> Self {
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
