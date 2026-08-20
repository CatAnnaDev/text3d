use std::ops::Range;
use std::path::{Path, PathBuf};

pub const TAB_WIDTH: usize = 4;

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
        let mut out = self.lines.join("\n");
        out.push('\n');
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

    fn byte_of(&self, line: usize, col: usize) -> usize {
        self.lines[line]
            .char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[line].len())
    }

    pub fn insert_char(&mut self, ch: char) {
        let at = self.byte_of(self.cursor_line, self.cursor_col);
        self.lines[self.cursor_line].insert(at, ch);
        self.cursor_col += 1;
        self.goal_col = self.cursor_col;
        self.modified = true;
        self.version += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        let mut column = self.cursor_col;
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => {
                    self.insert_newline();
                    column = 0;
                }
                '\t' => {
                    let pad = TAB_WIDTH - (column % TAB_WIDTH);
                    for _ in 0..pad {
                        self.insert_char(' ');
                    }
                    column += pad;
                }
                _ if ch.is_control() => {}
                _ => {
                    self.insert_char(ch);
                    column += 1;
                }
            }
        }
    }

    pub fn current_line(&self) -> &str {
        &self.lines[self.cursor_line]
    }

    pub fn delete_line(&mut self) {
        if self.lines.len() == 1 {
            self.lines[0].clear();
        } else {
            self.lines.remove(self.cursor_line);
            self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        }
        self.cursor_col = 0;
        self.goal_col = 0;
        self.modified = true;
        self.version += 1;
    }

    pub fn insert_newline(&mut self) {
        let at = self.byte_of(self.cursor_line, self.cursor_col);
        let tail = self.lines[self.cursor_line].split_off(at);
        self.lines.insert(self.cursor_line + 1, tail);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.goal_col = 0;
        self.modified = true;
        self.version += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let at = self.byte_of(self.cursor_line, self.cursor_col - 1);
            self.lines[self.cursor_line].remove(at);
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let tail = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.line_chars(self.cursor_line);
            self.lines[self.cursor_line].push_str(&tail);
        } else {
            return;
        }
        self.goal_col = self.cursor_col;
        self.modified = true;
        self.version += 1;
    }

    pub fn delete(&mut self) {
        let len = self.line_chars(self.cursor_line);
        if self.cursor_col < len {
            let at = self.byte_of(self.cursor_line, self.cursor_col);
            self.lines[self.cursor_line].remove(at);
        } else if self.cursor_line + 1 < self.lines.len() {
            let tail = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&tail);
        } else {
            return;
        }
        self.modified = true;
        self.version += 1;
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.line_chars(self.cursor_line);
        }
        self.goal_col = self.cursor_col;
    }

    pub fn move_right(&mut self) {
        if self.cursor_col < self.line_chars(self.cursor_line) {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.goal_col = self.cursor_col;
    }

    pub fn move_vertical(&mut self, delta: isize) {
        let target = self.cursor_line as isize + delta;
        let target = target.clamp(0, self.lines.len() as isize - 1) as usize;
        self.cursor_line = target;
        self.cursor_col = self.goal_col.min(self.line_chars(target));
    }

    pub fn move_home(&mut self) {
        let line = &self.lines[self.cursor_line];
        let indent = line.chars().take_while(|c| *c == ' ').count();
        self.cursor_col = if self.cursor_col == indent { 0 } else { indent };
        self.goal_col = self.cursor_col;
    }

    pub fn move_end(&mut self) {
        self.cursor_col = self.line_chars(self.cursor_line);
        self.goal_col = self.cursor_col;
    }

    pub fn move_document(&mut self, to_end: bool) {
        if to_end {
            self.cursor_line = self.lines.len() - 1;
            self.cursor_col = self.line_chars(self.cursor_line);
        } else {
            self.cursor_line = 0;
            self.cursor_col = 0;
        }
        self.goal_col = self.cursor_col;
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

pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
