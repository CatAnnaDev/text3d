use std::collections::VecDeque;

use crate::text::Cursor;

pub struct Change {
    pub start: Cursor,
    pub removed: String,
    pub inserted: String,
    pub cursor_before: Cursor,
    pub cursor_after: Cursor,
}

pub struct History {
    entries: VecDeque<Change>,
    applied: usize,
    limit: usize,
}

impl History {
    pub fn new(limit: usize) -> History {
        History {
            entries: VecDeque::new(),
            applied: 0,
            limit: limit.max(1),
        }
    }

    pub fn record(&mut self, change: Change, coalesce: bool) {
        self.entries.truncate(self.applied);
        if coalesce
            && let Some(previous) = self.entries.back_mut()
            && extends(previous, &change)
        {
            previous.inserted.push_str(&change.inserted);
            previous.cursor_after = change.cursor_after;
            return;
        }
        self.entries.push_back(change);
        while self.entries.len() > self.limit {
            self.entries.pop_front();
        }
        self.applied = self.entries.len();
    }

    pub fn undo(&mut self) -> Option<&Change> {
        if self.applied == 0 {
            return None;
        }
        self.applied -= 1;
        self.entries.get(self.applied)
    }

    pub fn redo(&mut self) -> Option<&Change> {
        if self.applied >= self.entries.len() {
            return None;
        }
        let at = self.applied;
        self.applied += 1;
        self.entries.get(at)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.applied = 0;
    }

    pub fn can_undo(&self) -> bool {
        self.applied > 0
    }

    pub fn can_redo(&self) -> bool {
        self.applied < self.entries.len()
    }
}

fn extends(previous: &Change, next: &Change) -> bool {
    if !previous.removed.is_empty() || !next.removed.is_empty() {
        return false;
    }
    let mut typed = next.inserted.chars();
    let (Some(added), None) = (typed.next(), typed.next()) else {
        return false;
    };
    if added == '\n' {
        return false;
    }
    let Some(last) = previous.inserted.chars().next_back() else {
        return false;
    };
    if last == '\n' || previous.cursor_after != next.start {
        return false;
    }
    !added.is_whitespace() || last == added
}
