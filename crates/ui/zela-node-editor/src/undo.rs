//! Undo/redo stacks holding inverse [`Command`](crate::Command)s by value.
//!
//! Commands are not `Clone` (they may carry `Box<dyn Node>`), so entries are
//! moved between the two stacks: undoing pops an inverse from `undo` and the
//! editor pushes its own inverse onto `redo`.

use crate::command::Command;

/// Two LIFO stacks of inverse commands.
#[derive(Default)]
pub struct UndoStack {
    undo: Vec<Command>,
    redo: Vec<Command>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the inverse of a successful user action.
    pub fn push(&mut self, inverse: Command) {
        self.undo.push(inverse);
    }

    /// Pop the next inverse command to apply for an undo.
    pub fn undo(&mut self) -> Option<Command> {
        self.undo.pop()
    }

    /// Push the inverse produced by applying a redo entry (so it can be
    /// undone again).
    pub fn push_redo(&mut self, command: Command) {
        self.redo.push(command);
    }

    /// Pop the next command to reapply for a redo.
    pub fn redo(&mut self) -> Option<Command> {
        self.redo.pop()
    }

    /// Drop all redo entries (called whenever a new action lands).
    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    /// Peek the most recent inverse without popping it (used to coalesce
    /// high-frequency commands like per-frame node drags).
    pub fn last(&self) -> Option<&Command> {
        self.undo.last()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
