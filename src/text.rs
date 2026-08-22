use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::history::{Change, History};

pub const TAB_WIDTH: usize = 4;

const HISTORY_LIMIT: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Cursor {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy)]
pub struct Selection {
    pub anchor: Cursor,
    pub head: Cursor,
}

impl Selection {
    pub fn range(&self) -> (Cursor, Cursor) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Copy)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub struct TextBuffer {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub path: Option<PathBuf>,
    pub modified: bool,
    pub status: String,
    goal_col: usize,
    version: u64,
    source: String,
    line_starts: Vec<usize>,
    source_version: u64,
    anchor: Option<Cursor>,
    history: History,
    typing: bool,
    pending: String,
    matches: Vec<Match>,
    needle: Vec<char>,
    current: Option<usize>,
}

impl TextBuffer {
    pub fn from_str(text: &str, path: Option<PathBuf>) -> Self {
        let lines: Vec<String> = text
            .replace('\r', "")
            .split('\n')
            .map(expand_tabs)
            .collect();
        let lines = if lines.is_empty() { vec![String::new()] } else { lines };
        TextBuffer {
            lines,
            cursor_line: 0,
            cursor_col: 0,
            path,
            modified: false,
            status: String::new(),
            goal_col: 0,
            version: 1,
            source: String::new(),
            line_starts: Vec::new(),
            source_version: 0,
            anchor: None,
            history: History::new(HISTORY_LIMIT),
            typing: false,
            pending: String::new(),
            matches: Vec::new(),
            needle: Vec::new(),
            current: None,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn sync(&mut self) {
        if self.source_version == self.version {
            return;
        }
        self.source_version = self.version;
        self.source.clear();
        self.line_starts.clear();
        self.line_starts.reserve(self.lines.len());
        for (index, line) in self.lines.iter().enumerate() {
            self.line_starts.push(self.source.len());
            self.source.push_str(line);
            if index + 1 < self.lines.len() {
                self.source.push('\n');
            }
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line).copied().unwrap_or(self.source.len())
    }

    pub fn byte_range(&self, lines: Range<usize>) -> Range<usize> {
        self.line_start(lines.start)..self.line_start(lines.end)
    }

    pub fn word_prefix(&self) -> &str {
        let line = &self.lines[self.cursor_line];
        let end = line
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let start = line[..end]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word_char(*c))
            .last()
            .map(|(i, _)| i)
            .unwrap_or(end);
        &line[start..end]
    }

    pub fn open(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_str(&text, Some(path.to_path_buf())),
            Err(err) => {
                let mut buffer = Self::from_str("", Some(path.to_path_buf()));
                buffer.status = format!("nouveau fichier ({err})");
                buffer
            }
        }
    }

    pub fn save(&mut self) {
        let path = match &self.path {
            Some(path) => path.clone(),
            None => PathBuf::from("untitled.txt"),
        };
        let out = self.lines.join("\n");
        match std::fs::write(&path, out) {
            Ok(()) => {
                self.modified = false;
                self.path = Some(path.clone());
                self.status = format!("enregistre {}", path.display());
            }
            Err(err) => self.status = format!("echec ecriture: {err}"),
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_chars(&self, index: usize) -> usize {
        self.lines[index].chars().count()
    }

    pub fn current_line(&self) -> &str {
        &self.lines[self.cursor_line]
    }

    pub fn cursor(&self) -> Cursor {
        Cursor { line: self.cursor_line, column: self.cursor_col }
    }

    fn clamped(&self, cursor: Cursor) -> Cursor {
        let line = cursor.line.min(self.lines.len() - 1);
        Cursor { line, column: cursor.column.min(self.line_chars(line)) }
    }

    fn place(&mut self, cursor: Cursor, extend: bool) {
        let cursor = self.clamped(cursor);
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor());
            }
        } else {
            self.anchor = None;
        }
        self.cursor_line = cursor.line;
        self.cursor_col = cursor.column;
        self.typing = false;
    }

    pub fn set_cursor(&mut self, cursor: Cursor, extend: bool) {
        self.place(cursor, extend);
        self.goal_col = self.cursor_col;
    }

    pub fn selection(&self) -> Option<Selection> {
        let anchor = self.anchor?;
        let head = self.cursor();
        if anchor == head {
            return None;
        }
        Some(Selection { anchor, head })
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        self.anchor = Some(Cursor { line: 0, column: 0 });
        self.cursor_line = last;
        self.cursor_col = self.line_chars(last);
        self.goal_col = self.cursor_col;
        self.typing = false;
    }

    pub fn select_word_at(&mut self, cursor: Cursor) {
        let cursor = self.clamped(cursor);
        let count = self.line_chars(cursor.line);
        if count == 0 {
            self.set_cursor(cursor, false);
            return;
        }
        let line = &self.lines[cursor.line];
        let index = cursor.column.min(count - 1);
        let class = class_of(line.chars().nth(index).unwrap_or(' '));
        let mut start = index;
        for ch in line[..byte_of(line, index)].chars().rev() {
            if class_of(ch) != class {
                break;
            }
            start -= 1;
        }
        let mut end = index + 1;
        for ch in line.chars().skip(end) {
            if class_of(ch) != class {
                break;
            }
            end += 1;
        }
        self.anchor = Some(Cursor { line: cursor.line, column: start });
        self.cursor_line = cursor.line;
        self.cursor_col = end;
        self.goal_col = end;
        self.typing = false;
    }

    pub fn select_line_at(&mut self, line: usize) {
        let line = line.min(self.lines.len() - 1);
        self.anchor = Some(Cursor { line, column: 0 });
        if line + 1 < self.lines.len() {
            self.cursor_line = line + 1;
            self.cursor_col = 0;
        } else {
            self.cursor_line = line;
            self.cursor_col = self.line_chars(line);
        }
        self.goal_col = self.cursor_col;
        self.typing = false;
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?.range();
        let mut out = String::new();
        collect_range(&self.lines, start, end, &mut out);
        Some(out)
    }

    pub fn selection_on_line(&self, line: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection()?.range();
        if line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.column } else { 0 };
        let to = if line == end.line { end.column } else { self.line_chars(line) + 1 };
        if from >= to { None } else { Some((from, to)) }
    }

    pub fn delete_selection(&mut self) -> bool {
        let Some(selection) = self.selection() else {
            return false;
        };
        let (start, end) = selection.range();
        self.edit(start, end, "", false);
        true
    }

    fn span(&self) -> (Cursor, Cursor) {
        match self.selection() {
            Some(selection) => selection.range(),
            None => {
                let cursor = self.clamped(self.cursor());
                (cursor, cursor)
            }
        }
    }

    fn splice_edit(&mut self, start: Cursor, end: Cursor, insert: &str, coalesce: bool) {
        let start = self.clamped(start);
        let end = self.clamped(end).max(start);
        let mut removed = String::new();
        collect_range(&self.lines, start, end, &mut removed);
        if removed.is_empty() && insert.is_empty() {
            return;
        }
        let cursor_before = self.cursor();
        splice(&mut self.lines, start, end, insert);
        let cursor_after = advance(start, insert);
        let merge = coalesce && self.typing;
        self.history.record(
            Change {
                start,
                removed,
                inserted: insert.to_string(),
                cursor_before,
                cursor_after,
            },
            merge,
        );
        self.cursor_line = cursor_after.line;
        self.cursor_col = cursor_after.column;
        self.goal_col = cursor_after.column;
        self.anchor = None;
        self.typing = coalesce;
        self.modified = true;
        self.version += 1;
    }

    fn edit(&mut self, start: Cursor, end: Cursor, insert: &str, coalesce: bool) {
        self.splice_edit(start, end, insert, coalesce);
        self.clear_matches();
    }

    pub fn insert_char(&mut self, ch: char) {
        let (start, end) = self.span();
        let mut encoded = [0u8; 4];
        let text = ch.encode_utf8(&mut encoded);
        self.edit(start, end, text, start == end);
    }

    pub fn insert_str(&mut self, text: &str) {
        let (start, end) = self.span();
        self.edit(start, end, text, false);
    }

    pub fn insert_text(&mut self, text: &str) {
        let (start, end) = self.span();
        let mut pending = std::mem::take(&mut self.pending);
        pending.clear();
        pending.reserve(text.len());
        let mut column = start.column;
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => {
                    pending.push('\n');
                    column = 0;
                }
                '\t' => {
                    let pad = TAB_WIDTH - (column % TAB_WIDTH);
                    pending.extend(std::iter::repeat_n(' ', pad));
                    column += pad;
                }
                _ if ch.is_control() => {}
                _ => {
                    pending.push(ch);
                    column += 1;
                }
            }
        }
        self.edit(start, end, &pending, false);
        self.pending = pending;
    }

    pub fn insert_newline(&mut self) {
        let (start, end) = self.span();
        self.edit(start, end, "\n", false);
    }

    pub fn delete_line(&mut self) {
        let line = self.cursor_line.min(self.lines.len() - 1);
        let (start, end) = if line + 1 < self.lines.len() {
            (Cursor { line, column: 0 }, Cursor { line: line + 1, column: 0 })
        } else if line > 0 {
            (
                Cursor { line: line - 1, column: self.line_chars(line - 1) },
                Cursor { line, column: self.line_chars(line) },
            )
        } else {
            (Cursor { line: 0, column: 0 }, Cursor { line: 0, column: self.line_chars(0) })
        };
        self.edit(start, end, "", false);
        let line = self.cursor_line.min(self.lines.len() - 1);
        self.set_cursor(Cursor { line, column: 0 }, false);
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.clamped(self.cursor());
        let start = if end.column > 0 {
            Cursor { line: end.line, column: end.column - 1 }
        } else if end.line > 0 {
            Cursor { line: end.line - 1, column: self.line_chars(end.line - 1) }
        } else {
            return;
        };
        self.edit(start, end, "", false);
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let start = self.clamped(self.cursor());
        let end = if start.column < self.line_chars(start.line) {
            Cursor { line: start.line, column: start.column + 1 }
        } else if start.line + 1 < self.lines.len() {
            Cursor { line: start.line + 1, column: 0 }
        } else {
            return;
        };
        self.edit(start, end, "", false);
    }

    pub fn delete_word_left(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.clamped(self.cursor());
        let start = self.word_target(false);
        if start == end {
            return;
        }
        self.edit(start, end, "", false);
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && let Some(selection) = self.selection() {
            let (start, _) = selection.range();
            self.set_cursor(start, false);
            return;
        }
        let cursor = self.clamped(self.cursor());
        let target = if cursor.column > 0 {
            Cursor { line: cursor.line, column: cursor.column - 1 }
        } else if cursor.line > 0 {
            Cursor { line: cursor.line - 1, column: self.line_chars(cursor.line - 1) }
        } else {
            cursor
        };
        self.set_cursor(target, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && let Some(selection) = self.selection() {
            let (_, end) = selection.range();
            self.set_cursor(end, false);
            return;
        }
        let cursor = self.clamped(self.cursor());
        let target = if cursor.column < self.line_chars(cursor.line) {
            Cursor { line: cursor.line, column: cursor.column + 1 }
        } else if cursor.line + 1 < self.lines.len() {
            Cursor { line: cursor.line + 1, column: 0 }
        } else {
            cursor
        };
        self.set_cursor(target, extend);
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let target = (self.cursor_line as isize + delta).clamp(0, self.lines.len() as isize - 1);
        let line = target as usize;
        let column = self.goal_col.min(self.line_chars(line));
        self.place(Cursor { line, column }, extend);
    }

    pub fn move_word(&mut self, forward: bool, extend: bool) {
        let target = self.word_target(forward);
        self.set_cursor(target, extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        let cursor = self.clamped(self.cursor());
        let indent = self.lines[cursor.line].chars().take_while(|c| *c == ' ').count();
        let column = if cursor.column == indent { 0 } else { indent };
        self.set_cursor(Cursor { line: cursor.line, column }, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        let line = self.cursor_line.min(self.lines.len() - 1);
        self.set_cursor(Cursor { line, column: self.line_chars(line) }, extend);
    }

    pub fn move_document(&mut self, to_end: bool, extend: bool) {
        let target = if to_end {
            let line = self.lines.len() - 1;
            Cursor { line, column: self.line_chars(line) }
        } else {
            Cursor { line: 0, column: 0 }
        };
        self.set_cursor(target, extend);
    }

    fn word_target(&self, forward: bool) -> Cursor {
        let cursor = self.clamped(self.cursor());
        let line = &self.lines[cursor.line];
        let mut column = cursor.column;
        let mut seen = false;
        if forward {
            if column >= self.line_chars(cursor.line) {
                if cursor.line + 1 < self.lines.len() {
                    return Cursor { line: cursor.line + 1, column: 0 };
                }
                return cursor;
            }
            for ch in line.chars().skip(column) {
                if is_word_char(ch) {
                    seen = true;
                } else if seen {
                    break;
                }
                column += 1;
            }
        } else {
            if column == 0 {
                if cursor.line > 0 {
                    return Cursor {
                        line: cursor.line - 1,
                        column: self.line_chars(cursor.line - 1),
                    };
                }
                return cursor;
            }
            for ch in line[..byte_of(line, column)].chars().rev() {
                if is_word_char(ch) {
                    seen = true;
                } else if seen {
                    break;
                }
                column -= 1;
            }
        }
        Cursor { line: cursor.line, column }
    }

    pub fn undo(&mut self) -> bool {
        let Some(change) = self.history.undo() else {
            return false;
        };
        let end = advance(change.start, &change.inserted);
        splice(&mut self.lines, change.start, end, &change.removed);
        let cursor = change.cursor_before;
        self.cursor_line = cursor.line;
        self.cursor_col = cursor.column;
        self.goal_col = cursor.column;
        self.anchor = None;
        self.typing = false;
        self.modified = true;
        self.version += 1;
        self.clear_matches();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(change) = self.history.redo() else {
            return false;
        };
        let end = advance(change.start, &change.removed);
        splice(&mut self.lines, change.start, end, &change.inserted);
        let cursor = change.cursor_after;
        self.cursor_line = cursor.line;
        self.cursor_col = cursor.column;
        self.goal_col = cursor.column;
        self.anchor = None;
        self.typing = false;
        self.modified = true;
        self.version += 1;
        self.clear_matches();
        true
    }

    pub fn find_all(&mut self, needle: &str, case_sensitive: bool) {
        self.matches.clear();
        self.current = None;
        self.needle.clear();
        if needle.is_empty() {
            return;
        }
        self.needle.extend(needle.chars().map(|ch| fold(ch, case_sensitive)));
        let pattern = &self.needle;
        let found = &mut self.matches;
        for (index, line) in self.lines.iter().enumerate() {
            scan_line(line, pattern, case_sensitive, index, found);
        }
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn match_on_line(&self, line: usize) -> &[Match] {
        let start = self.matches.partition_point(|found| found.line < line);
        let end = start + self.matches[start..].partition_point(|found| found.line == line);
        &self.matches[start..end]
    }

    pub fn focus_match(&mut self, forward: bool) -> bool {
        let count = self.matches.len();
        if count == 0 {
            return false;
        }
        let next = match self.current {
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
            None => {
                let cursor = self.cursor();
                let at = self
                    .matches
                    .partition_point(|found| (found.line, found.start) < (cursor.line, cursor.column));
                if forward { at % count } else { (at + count - 1) % count }
            }
        };
        self.current = Some(next);
        let found = self.matches[next];
        self.place(Cursor { line: found.line, column: found.start }, false);
        self.goal_col = self.cursor_col;
        true
    }

    pub fn current_match(&self) -> Option<usize> {
        self.current
    }

    pub fn replace_current(&mut self, with: &str) -> bool {
        let Some(index) = self.current else {
            return false;
        };
        let Some(found) = self.matches.get(index).copied() else {
            self.current = None;
            return false;
        };
        let start = Cursor { line: found.line, column: found.start };
        let end = Cursor { line: found.line, column: found.end };
        self.splice_edit(start, end, with, false);
        let breaks = count_breaks(with);
        let tail = advance(start, with);
        for later in self.matches.iter_mut().skip(index + 1) {
            if later.line != found.line {
                later.line += breaks;
                continue;
            }
            let width = later.end - later.start;
            let offset = later.start - found.end;
            later.line = tail.line;
            later.start = tail.column + offset;
            later.end = later.start + width;
        }
        self.matches.remove(index);
        self.current = None;
        true
    }

    pub fn replace_all(&mut self, with: &str) -> usize {
        let count = self.matches.len();
        if count == 0 {
            return 0;
        }
        let first = self.matches[0];
        let last = self.matches[count - 1];
        let start = Cursor { line: first.line, column: first.start };
        let end = Cursor { line: last.line, column: last.end };
        let mut built = String::with_capacity(with.len() * count + 64);
        let mut position = start;
        for found in &self.matches {
            collect_range(
                &self.lines,
                position,
                Cursor { line: found.line, column: found.start },
                &mut built,
            );
            built.push_str(with);
            position = Cursor { line: found.line, column: found.end };
        }
        self.edit(start, end, &built, false);
        count
    }

    pub fn clear_matches(&mut self) {
        self.matches.clear();
        self.current = None;
    }
}

fn byte_of(line: &str, column: usize) -> usize {
    line.char_indices().nth(column).map(|(i, _)| i).unwrap_or(line.len())
}

fn count_breaks(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
}

fn advance(start: Cursor, text: &str) -> Cursor {
    let mut cursor = start;
    for ch in text.chars() {
        if ch == '\n' {
            cursor.line += 1;
            cursor.column = 0;
        } else {
            cursor.column += 1;
        }
    }
    cursor
}

fn collect_range(lines: &[String], start: Cursor, end: Cursor, out: &mut String) {
    if start >= end {
        return;
    }
    let first = &lines[start.line];
    if start.line == end.line {
        out.push_str(&first[byte_of(first, start.column)..byte_of(first, end.column)]);
        return;
    }
    out.push_str(&first[byte_of(first, start.column)..]);
    out.push('\n');
    for line in &lines[start.line + 1..end.line] {
        out.push_str(line);
        out.push('\n');
    }
    let last = &lines[end.line];
    out.push_str(&last[..byte_of(last, end.column)]);
}

fn splice(lines: &mut Vec<String>, start: Cursor, end: Cursor, insert: &str) {
    let start_byte = byte_of(&lines[start.line], start.column);
    let end_byte = byte_of(&lines[end.line], end.column);
    let breaks = count_breaks(insert);
    if start.line == end.line && breaks == 0 {
        lines[start.line].replace_range(start_byte..end_byte, insert);
        return;
    }
    let tail = lines[end.line].split_off(end_byte);
    if end.line > start.line {
        lines.drain(start.line + 1..=end.line);
    }
    let head = &mut lines[start.line];
    head.truncate(start_byte);
    let mut parts = insert.split('\n');
    if let Some(part) = parts.next() {
        head.push_str(part);
    }
    if breaks > 0 {
        let at = start.line + 1;
        lines.reserve(breaks);
        lines.splice(at..at, parts.map(String::from));
    }
    lines[start.line + breaks].push_str(&tail);
}

fn fold(ch: char, case_sensitive: bool) -> char {
    if case_sensitive {
        ch
    } else {
        ch.to_lowercase().next().unwrap_or(ch)
    }
}

fn match_width(hay: &str, pattern: &[char], case_sensitive: bool) -> Option<usize> {
    let mut chars = hay.chars();
    let mut width = 0;
    for wanted in pattern {
        let ch = chars.next()?;
        if fold(ch, case_sensitive) != *wanted {
            return None;
        }
        width += ch.len_utf8();
    }
    Some(width)
}

fn scan_line(line: &str, pattern: &[char], case_sensitive: bool, index: usize, out: &mut Vec<Match>) {
    if pattern.is_empty() {
        return;
    }
    let mut byte = 0;
    let mut column = 0;
    while byte < line.len() {
        match match_width(&line[byte..], pattern, case_sensitive) {
            Some(width) => {
                out.push(Match { line: index, start: column, end: column + pattern.len() });
                byte += width;
                column += pattern.len();
            }
            None => {
                byte += line[byte..].chars().next().map(char::len_utf8).unwrap_or(1);
                column += 1;
            }
        }
    }
}

fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let pad = TAB_WIDTH - (column % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', pad));
            column += pad;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    out
}

fn class_of(ch: char) -> u8 {
    if is_word_char(ch) {
        0
    } else if ch.is_whitespace() {
        1
    } else {
        2
    }
}

pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
