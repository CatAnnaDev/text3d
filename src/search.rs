use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const BINARY_PROBE: usize = 8 * 1024;
const MAX_HITS: usize = 5000;
const MAX_WORKERS: usize = 8;
const PREVIEW_CHARS: usize = 200;
const PREVIEW_BEFORE: usize = 60;
const PREVIEW_SCAN: usize = 1024;
const POLL_MESSAGES: usize = 256;

const SWAR_ONES: u64 = 0x0101_0101_0101_0101;
const SWAR_HIGH: u64 = 0x8080_8080_8080_8080;
const SWAR_CASE: u64 = 0x2020_2020_2020_2020;

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub preview: String,
}

#[inline]
fn is_char_start(byte: u8) -> bool {
    byte & 0xC0 != 0x80
}

#[inline]
fn is_word_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || byte >= 0x80
}

fn find_byte(data: &[u8], from: usize, target: u8, fold: bool) -> Option<usize> {
    let broadcast = SWAR_ONES.wrapping_mul(target as u64);
    let mut index = from;
    while index + 8 <= data.len() {
        let Ok(block) = <[u8; 8]>::try_from(&data[index..index + 8]) else {
            break;
        };
        let mut chunk = u64::from_le_bytes(block);
        if fold {
            chunk |= SWAR_CASE;
        }
        let diff = chunk ^ broadcast;
        let hit = diff.wrapping_sub(SWAR_ONES) & !diff & SWAR_HIGH;
        if hit != 0 {
            return Some(index + (hit.trailing_zeros() >> 3) as usize);
        }
        index += 8;
    }
    while index < data.len() {
        let byte = if fold {
            data[index] | 0x20
        } else {
            data[index]
        };
        if byte == target {
            return Some(index);
        }
        index += 1;
    }
    None
}

struct Query {
    files: Arc<Vec<PathBuf>>,
    needle: Vec<u8>,
    chars: usize,
    case_sensitive: bool,
    whole_word: bool,
    fold_first: bool,
}

impl Query {
    fn matches_at(&self, data: &[u8], at: usize) -> bool {
        let end = at + self.needle.len();
        if end > data.len() || !is_char_start(data[at]) {
            return false;
        }
        if self.case_sensitive {
            if data[at..end] != self.needle[..] {
                return false;
            }
        } else {
            for (offset, &wanted) in self.needle.iter().enumerate() {
                if data[at + offset].to_ascii_lowercase() != wanted {
                    return false;
                }
            }
        }
        if self.whole_word {
            if at > 0 && is_word_byte(data[at - 1]) {
                return false;
            }
            if end < data.len() && is_word_byte(data[end]) {
                return false;
            }
        }
        true
    }
}

fn build_preview(data: &[u8], line_start: usize, at: usize, out: &mut String) {
    let mut start = at;
    let mut back = 0;
    while start > line_start && back < PREVIEW_BEFORE {
        start -= 1;
        while start > line_start && !is_char_start(data[start]) {
            start -= 1;
        }
        back += 1;
    }
    let limit = (at + PREVIEW_SCAN).min(data.len());
    let mut end = at;
    while end < limit && data[end] != b'\n' {
        end += 1;
    }
    while end > start && end < data.len() && !is_char_start(data[end]) {
        end -= 1;
    }
    out.clear();
    out.reserve(PREVIEW_CHARS);
    let mut written = 0;
    for character in String::from_utf8_lossy(&data[start..end]).chars() {
        if written == PREVIEW_CHARS {
            break;
        }
        if (character as u32) < 0x20 || character == '\u{7f}' {
            out.push(' ');
        } else {
            out.push(character);
        }
        written += 1;
    }
}

fn scan_data(path: &Path, data: &[u8], query: &Query, out: &mut Vec<Hit>) {
    let mut newline_cursor = 0usize;
    let mut counted = 0usize;
    let mut line = 0usize;
    let mut line_start = 0usize;
    let mut column = 0usize;
    let mut at = 0usize;
    while let Some(found) = find_byte(data, at, query.needle[0], query.fold_first) {
        if !query.matches_at(data, found) {
            at = found + 1;
            continue;
        }
        while let Some(newline) = find_byte(&data[..found], newline_cursor, b'\n', false) {
            line += 1;
            line_start = newline + 1;
            newline_cursor = newline + 1;
            counted = line_start;
            column = 0;
        }
        newline_cursor = found;
        while counted < found {
            if is_char_start(data[counted]) {
                column += 1;
            }
            counted += 1;
        }
        let mut preview = String::new();
        build_preview(data, line_start, found, &mut preview);
        out.push(Hit {
            path: path.to_path_buf(),
            line,
            column,
            length: query.chars,
            preview,
        });
        if out.len() >= MAX_HITS {
            return;
        }
        at = found + query.needle.len();
    }
}

fn read_file(path: &Path, buffer: &mut Vec<u8>) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    match file.metadata() {
        Ok(data) => {
            if data.len() > MAX_FILE_BYTES {
                return false;
            }
        }
        Err(_) => return false,
    }
    buffer.clear();
    if file.take(MAX_FILE_BYTES).read_to_end(buffer).is_err() {
        return false;
    }
    let probe = buffer.len().min(BINARY_PROBE);
    find_byte(&buffer[..probe], 0, 0, false).is_none()
}

struct Shared {
    cursor: AtomicUsize,
    done: AtomicUsize,
    found: AtomicUsize,
    cancel: AtomicBool,
    capped: AtomicBool,
}

enum Message {
    Hits { file: u32, hits: Vec<Hit> },
    Finished,
}

fn worker(query: Arc<Query>, shared: Arc<Shared>, sender: Sender<Message>) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut hits: Vec<Hit> = Vec::new();
    let total = query.files.len();
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let index = shared.cursor.fetch_add(1, Ordering::Relaxed);
        if index >= total {
            break;
        }
        let path = &query.files[index];
        if read_file(path, &mut buffer) {
            hits.clear();
            scan_data(path, &buffer, &query, &mut hits);
            if !hits.is_empty() {
                let reserved = shared.found.fetch_add(hits.len(), Ordering::Relaxed);
                if reserved >= MAX_HITS {
                    shared.capped.store(true, Ordering::Relaxed);
                    shared.cancel.store(true, Ordering::Relaxed);
                    break;
                }
                if reserved + hits.len() > MAX_HITS {
                    hits.truncate(MAX_HITS - reserved);
                    shared.capped.store(true, Ordering::Relaxed);
                    shared.cancel.store(true, Ordering::Relaxed);
                }
                let batch = std::mem::take(&mut hits);
                if sender
                    .send(Message::Hits {
                        file: index as u32,
                        hits: batch,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
        shared.done.fetch_add(1, Ordering::Relaxed);
    }
    let _ = sender.send(Message::Finished);
}

pub struct ProjectSearch {
    hits: Vec<Hit>,
    hit_file: Vec<u32>,
    needle: String,
    running: bool,
    capped: bool,
    total: usize,
    pending: usize,
    shared: Option<Arc<Shared>>,
    receiver: Option<Receiver<Message>>,
    handles: Vec<JoinHandle<()>>,
    retiring: Vec<JoinHandle<()>>,
    cached: Option<Arc<Vec<PathBuf>>>,
    cached_source: (usize, usize),
}

impl ProjectSearch {
    pub fn new() -> ProjectSearch {
        ProjectSearch {
            hits: Vec::new(),
            hit_file: Vec::new(),
            needle: String::new(),
            running: false,
            capped: false,
            total: 0,
            pending: 0,
            shared: None,
            receiver: None,
            handles: Vec::new(),
            retiring: Vec::new(),
            cached: None,
            cached_source: (0, 0),
        }
    }

    pub fn start(
        &mut self,
        files: &[PathBuf],
        needle: &str,
        case_sensitive: bool,
        whole_word: bool,
    ) {
        self.cancel();
        self.hits.clear();
        self.hit_file.clear();
        self.needle.clear();
        self.needle.push_str(needle);
        self.capped = false;
        self.total = files.len();
        if needle.is_empty() || files.is_empty() {
            self.total = 0;
            return;
        }
        let mut bytes: Vec<u8> = needle.as_bytes().to_vec();
        if !case_sensitive {
            bytes.make_ascii_lowercase();
        }
        let fold_first = !case_sensitive && bytes[0].is_ascii_alphabetic();
        let query = Arc::new(Query {
            files: self.shared_files(files),
            chars: needle.chars().count(),
            needle: bytes,
            case_sensitive,
            whole_word,
            fold_first,
        });
        let shared = Arc::new(Shared {
            cursor: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            found: AtomicUsize::new(0),
            cancel: AtomicBool::new(false),
            capped: AtomicBool::new(false),
        });
        let parallel = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4);
        let count = parallel.clamp(1, MAX_WORKERS).min(files.len());
        let (sender, receiver) = channel();
        self.handles.reserve(count);
        for _ in 0..count {
            let query = Arc::clone(&query);
            let state = Arc::clone(&shared);
            let sender = sender.clone();
            match std::thread::Builder::new()
                .name(String::from("recherche"))
                .spawn(move || worker(query, state, sender))
            {
                Ok(handle) => self.handles.push(handle),
                Err(_) => break,
            }
        }
        drop(sender);
        self.pending = self.handles.len();
        if self.pending == 0 {
            self.total = 0;
            return;
        }
        self.shared = Some(shared);
        self.receiver = Some(receiver);
        self.running = true;
    }

    fn shared_files(&mut self, files: &[PathBuf]) -> Arc<Vec<PathBuf>> {
        let source = (files.as_ptr() as usize, files.len());
        if source == self.cached_source
            && let Some(cached) = &self.cached
            && cached.len() == files.len()
            && cached.first() == files.first()
            && cached.last() == files.last()
        {
            return Arc::clone(cached);
        }
        let owned = Arc::new(files.to_vec());
        self.cached = Some(Arc::clone(&owned));
        self.cached_source = source;
        owned
    }

    pub fn poll(&mut self) -> bool {
        self.reap();
        if !self.running {
            return false;
        }
        let mut changed = false;
        let mut budget = POLL_MESSAGES;
        while budget > 0 {
            let message = match &self.receiver {
                Some(receiver) => receiver.try_recv(),
                None => break,
            };
            match message {
                Ok(Message::Hits { file, hits }) => {
                    self.integrate(file, hits);
                    changed = true;
                }
                Ok(Message::Finished) => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.pending == 0 {
                        self.finish();
                        return true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finish();
                    return true;
                }
            }
            budget -= 1;
        }
        changed
    }

    fn finish(&mut self) {
        self.running = false;
        self.pending = 0;
        if let Some(shared) = &self.shared {
            self.capped = shared.capped.load(Ordering::Relaxed);
            shared.done.store(self.total, Ordering::Relaxed);
        }
        self.receiver = None;
        self.retiring.append(&mut self.handles);
        self.reap();
    }

    fn integrate(&mut self, file: u32, hits: Vec<Hit>) {
        if hits.is_empty() {
            return;
        }
        let position = self.hit_file.partition_point(|&index| index <= file);
        let count = hits.len();
        self.hits.splice(position..position, hits);
        self.hit_file
            .splice(position..position, std::iter::repeat_n(file, count));
    }

    fn reap(&mut self) {
        let mut index = 0;
        while index < self.retiring.len() {
            if self.retiring[index].is_finished() {
                let handle = self.retiring.swap_remove(index);
                let _ = handle.join();
            } else {
                index += 1;
            }
        }
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    pub fn progress(&self) -> (usize, usize) {
        match &self.shared {
            Some(shared) => (
                shared.done.load(Ordering::Relaxed).min(self.total),
                self.total,
            ),
            None => (self.total, self.total),
        }
    }

    pub fn cancel(&mut self) {
        if let Some(shared) = self.shared.take() {
            shared.cancel.store(true, Ordering::Relaxed);
            self.capped = shared.capped.load(Ordering::Relaxed);
        }
        self.receiver = None;
        self.running = false;
        self.pending = 0;
        self.retiring.append(&mut self.handles);
        self.reap();
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn needle(&self) -> &str {
        &self.needle
    }

    pub fn capped(&self) -> bool {
        self.capped
    }

    pub fn threads(&self) -> usize {
        self.handles.len() + self.retiring.len()
    }
}

impl Default for ProjectSearch {
    fn default() -> ProjectSearch {
        ProjectSearch::new()
    }
}

impl Drop for ProjectSearch {
    fn drop(&mut self) {
        self.cancel();
        for handle in self.retiring.drain(..) {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileMatch {
    pub path: PathBuf,
    pub display: String,
    pub score: u32,
}

const NAME_BONUS: u32 = 48;
const NAME_TIGHT: u32 = 32;
const CANDIDATE_FLOOR: usize = 96;
const CANDIDATE_SCALE: usize = 4;
const CANDIDATE_CEIL: usize = 384;
const CLASS_OTHER: u32 = 40;

fn class_bit(byte: u8) -> u32 {
    match byte {
        b'a'..=b'z' => (byte - b'a') as u32,
        b'0'..=b'9' => 26 + (byte - b'0') as u32,
        b'_' => 36,
        b'-' => 37,
        b'.' => 38,
        b'/' | b'\\' => 39,
        _ => CLASS_OTHER,
    }
}

fn mask_of(bytes: &[u8]) -> u64 {
    let mut mask = 0u64;
    for &byte in bytes {
        if !byte.is_ascii() {
            return u64::MAX;
        }
        mask |= 1u64 << class_bit(byte.to_ascii_lowercase());
    }
    mask
}

fn is_boundary_byte(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\' | b'_' | b'-' | b'.' | b' ')
}

fn prescore(fold: &[u8], needle: &[u8]) -> Option<u32> {
    let mut score = 0u32;
    let mut cursor = 0usize;
    let mut previous = usize::MAX;
    for &wanted in needle {
        let at = find_byte(fold, cursor, wanted, false)?;
        score += 16;
        if at == previous {
            score += 4;
        } else if at == 0 || is_boundary_byte(fold[at - 1]) {
            score += 8;
        }
        score = score.saturating_sub((at - cursor).min(8) as u32);
        previous = at + 1;
        cursor = at + 1;
    }
    Some(score)
}

pub struct FileFinder {
    paths: Vec<PathBuf>,
    text: String,
    fold: Vec<u8>,
    spans: Vec<(u32, u32)>,
    name_at: Vec<u32>,
    masks: Vec<u64>,
    matcher: Matcher,
    haystack: Vec<char>,
    needle: Vec<u8>,
    scored: Vec<(u32, u32)>,
    exotic: Vec<u32>,
    ranked: Vec<(u32, u32)>,
    results: Vec<FileMatch>,
    pattern: Pattern,
    count: usize,
}

impl FileFinder {
    pub fn new() -> FileFinder {
        FileFinder {
            paths: Vec::new(),
            text: String::new(),
            fold: Vec::new(),
            spans: Vec::new(),
            name_at: Vec::new(),
            masks: Vec::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            haystack: Vec::new(),
            needle: Vec::new(),
            scored: Vec::new(),
            exotic: Vec::new(),
            ranked: Vec::new(),
            results: Vec::new(),
            pattern: Pattern::default(),
            count: 0,
        }
    }

    pub fn set_files(&mut self, files: &[PathBuf], root: &Path) {
        self.paths.clear();
        self.text.clear();
        self.fold.clear();
        self.spans.clear();
        self.name_at.clear();
        self.masks.clear();
        self.count = 0;
        self.paths.reserve(files.len());
        self.spans.reserve(files.len());
        self.name_at.reserve(files.len());
        self.masks.reserve(files.len());
        self.text.reserve(files.len() * 32);
        for path in files {
            let relative = path.strip_prefix(root).unwrap_or(path);
            let start = self.text.len() as u32;
            match relative.to_str() {
                Some(text) => self.text.push_str(text),
                None => self.text.push_str(&relative.to_string_lossy()),
            }
            let end = self.text.len() as u32;
            let slice = &self.text.as_bytes()[start as usize..end as usize];
            let name = match slice
                .iter()
                .rposition(|&byte| byte == b'/' || byte == b'\\')
            {
                Some(index) => index as u32 + 1,
                None => 0,
            };
            self.fold
                .extend(slice.iter().map(|byte| byte.to_ascii_lowercase()));
            self.masks.push(mask_of(slice));
            self.spans.push((start, end));
            self.name_at.push(name);
            self.paths.push(path.clone());
        }
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn query(&mut self, pattern: &str, limit: usize) -> &[FileMatch] {
        self.count = 0;
        if limit == 0 || self.paths.is_empty() {
            return &self.results[..0];
        }
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            let count = limit.min(self.paths.len());
            for slot in 0..count {
                let (start, end) = self.spans[slot];
                let display = &self.text[start as usize..end as usize];
                let path = &self.paths[slot];
                assign(&mut self.results, slot, path, display, 0);
            }
            self.count = count;
            return &self.results[..count];
        }
        if trimmed.is_ascii() && !trimmed.bytes().any(is_special_byte) {
            self.query_atom(trimmed, limit);
        } else {
            self.query_pattern(trimmed);
        }
        self.finish(limit)
    }

    fn query_atom(&mut self, pattern: &str, limit: usize) {
        self.needle.clear();
        self.needle
            .extend(pattern.bytes().map(|byte| byte.to_ascii_lowercase()));
        let wanted = mask_of(self.needle.as_slice());
        let keep = (limit * CANDIDATE_SCALE).clamp(CANDIDATE_FLOOR, CANDIDATE_CEIL);
        self.scored.clear();
        self.exotic.clear();
        let trim_at = keep * 4;
        let mut floor = 0u32;
        for (index, &(start, end)) in self.spans.iter().enumerate() {
            let mask = self.masks[index];
            let entry = &self.fold[start as usize..end as usize];
            let score = if mask == u64::MAX {
                match prescore(entry, &self.needle) {
                    Some(value) => value,
                    None => {
                        if self.exotic.len() < CANDIDATE_CEIL {
                            self.exotic.push(index as u32);
                        }
                        continue;
                    }
                }
            } else {
                if mask & wanted != wanted {
                    continue;
                }
                match prescore(entry, &self.needle) {
                    Some(value) => value,
                    None => continue,
                }
            };
            if score < floor {
                continue;
            }
            self.scored.push((score, index as u32));
            if self.scored.len() >= trim_at {
                self.scored
                    .select_nth_unstable_by(keep, |a, b| b.0.cmp(&a.0));
                self.scored.truncate(keep);
                floor = self.scored.iter().map(|entry| entry.0).min().unwrap_or(0);
            }
        }
        if self.scored.len() > keep {
            self.scored
                .select_nth_unstable_by(keep, |a, b| b.0.cmp(&a.0));
            self.scored.truncate(keep);
        }
        let atom = Atom::new(
            pattern,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        );
        let Self {
            text,
            spans,
            name_at,
            matcher,
            haystack,
            scored,
            exotic,
            ranked,
            ..
        } = self;
        ranked.clear();
        for &index in exotic.iter().chain(scored.iter().map(|entry| &entry.1)) {
            let (start, end) = spans[index as usize];
            let display = &text[start as usize..end as usize];
            let full = match atom.score(Utf32Str::new(display, haystack), matcher) {
                Some(value) => u32::from(value),
                None => continue,
            };
            let offset = name_at[index as usize] as usize;
            let name = &display[offset..];
            let bonus = match atom.score(Utf32Str::new(name, haystack), matcher) {
                Some(value) => {
                    u32::from(value) + NAME_BONUS + NAME_TIGHT.saturating_sub(name.len() as u32)
                }
                None => 0,
            };
            ranked.push((full + bonus, index));
        }
    }

    fn query_pattern(&mut self, pattern: &str) {
        self.pattern
            .reparse(pattern, CaseMatching::Smart, Normalization::Smart);
        let mut required = 0u64;
        for word in pattern.split_whitespace() {
            let cleaned = word.trim_start_matches(['\'', '^']).trim_end_matches('$');
            if word.starts_with('!') || cleaned.is_empty() || !cleaned.is_ascii() {
                continue;
            }
            required |= mask_of(cleaned.as_bytes());
        }
        let Self {
            text,
            spans,
            name_at,
            masks,
            matcher,
            haystack,
            ranked,
            pattern: parsed,
            ..
        } = self;
        ranked.clear();
        for (index, &(start, end)) in spans.iter().enumerate() {
            let mask = masks[index];
            if mask != u64::MAX && mask & required != required {
                continue;
            }
            let display = &text[start as usize..end as usize];
            let full = match parsed.score(Utf32Str::new(display, haystack), matcher) {
                Some(value) => value,
                None => continue,
            };
            let offset = name_at[index] as usize;
            let name = &display[offset..];
            let bonus = match parsed.score(Utf32Str::new(name, haystack), matcher) {
                Some(value) => value + NAME_BONUS + NAME_TIGHT.saturating_sub(name.len() as u32),
                None => 0,
            };
            ranked.push((full + bonus, index as u32));
        }
    }

    fn finish(&mut self, limit: usize) -> &[FileMatch] {
        let Self {
            text,
            spans,
            paths,
            ranked,
            results,
            ..
        } = self;
        ranked.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    let left = spans[a.1 as usize];
                    let right = spans[b.1 as usize];
                    (left.1 - left.0).cmp(&(right.1 - right.0))
                })
                .then_with(|| a.1.cmp(&b.1))
        });
        let count = limit.min(ranked.len());
        for slot in 0..count {
            let (score, index) = ranked[slot];
            let (start, end) = spans[index as usize];
            let display = &text[start as usize..end as usize];
            assign(results, slot, &paths[index as usize], display, score);
        }
        self.count = count;
        &self.results[..count]
    }
}

impl Default for FileFinder {
    fn default() -> FileFinder {
        FileFinder::new()
    }
}

fn is_special_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'!' | b'^' | b'$' | b'\'' | b'\\')
}

fn assign(results: &mut Vec<FileMatch>, slot: usize, path: &Path, display: &str, score: u32) {
    match results.get_mut(slot) {
        Some(entry) => {
            entry.score = score;
            entry.display.clear();
            entry.display.push_str(display);
            let buffer = entry.path.as_mut_os_string();
            buffer.clear();
            buffer.push(path.as_os_str());
        }
        None => results.push(FileMatch {
            path: path.to_path_buf(),
            display: display.to_string(),
            score,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::{Duration, Instant};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn workspace(label: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut root = std::env::temp_dir();
        root.push(format!(
            "text3d_recherche_{}_{}_{}",
            std::process::id(),
            label,
            unique
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dossier de test");
        root
    }

    fn write(root: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("dossier parent");
        }
        std::fs::write(&path, content).expect("ecriture de test");
        path
    }

    fn run(files: &[PathBuf], needle: &str, case_sensitive: bool, whole_word: bool) -> Vec<Hit> {
        let mut search = ProjectSearch::new();
        search.start(files, needle, case_sensitive, whole_word);
        let deadline = Instant::now() + Duration::from_secs(20);
        while search.running() && Instant::now() < deadline {
            search.poll();
            std::thread::yield_now();
        }
        search.poll();
        assert!(!search.running(), "la recherche n a pas termine");
        search.hits().to_vec()
    }

    #[test]
    fn litteral_insensible_a_la_casse() {
        let root = workspace("casse");
        let file = write(&root, "a.rs", b"let Alpha = 1;\nlet alpha = 2;\nBETA\n");
        let hits = run(&[file], "alpha", false, false);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 0);
        assert_eq!(hits[0].column, 4);
        assert_eq!(hits[0].length, 5);
        assert_eq!(hits[1].line, 1);
        assert_eq!(hits[1].column, 4);
        let sensible = run(&[hits[0].path.clone()], "alpha", true, false);
        assert_eq!(sensible.len(), 1);
        assert_eq!(sensible[0].line, 1);
    }

    #[test]
    fn mot_entier() {
        let root = workspace("mot");
        let file = write(&root, "a.rs", b"value valued _value value_ (value)\n");
        let partiels = run(&[file.clone()], "value", false, false);
        assert_eq!(partiels.len(), 5);
        let entiers = run(&[file], "value", false, true);
        assert_eq!(entiers.len(), 2);
        assert_eq!(entiers[0].column, 0);
        assert_eq!(entiers[1].column, 28);
    }

    #[test]
    fn colonnes_en_caracteres_et_frontieres_utf8() {
        let root = workspace("utf8");
        let file = write(&root, "a.rs", "ééé_cible et café\n".as_bytes());
        let hits = run(&[file.clone()], "cible", false, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].column, 4);
        let ligne = "ééé_cible et café";
        let debut: String = ligne
            .chars()
            .skip(hits[0].column)
            .take(hits[0].length)
            .collect();
        assert_eq!(debut, "cible");
        let accents = run(&[file.clone()], "é", false, false);
        assert_eq!(accents.len(), 4);
        assert_eq!(accents[0].column, 0);
        assert_eq!(accents[3].column, 16);
        let sans_faux_positif = run(&[file], "e", false, false);
        assert_eq!(sans_faux_positif.len(), 2);
        assert_eq!(sans_faux_positif[0].column, 8);
        assert_eq!(sans_faux_positif[1].column, 10);
    }

    #[test]
    fn aucune_correspondance_au_milieu_d_un_caractere() {
        let root = workspace("milieu");
        let mut contenu: Vec<u8> = Vec::new();
        for _ in 0..200 {
            contenu.extend_from_slice("😀é中".as_bytes());
        }
        contenu.extend_from_slice("fin\n".as_bytes());
        let file = write(&root, "a.txt", &contenu);
        for motif in ["中", "😀", "é", "fin"] {
            let hits = run(&[file.clone()], motif, false, false);
            assert!(!hits.is_empty(), "motif {motif} introuvable");
            let texte = String::from_utf8(contenu.clone()).expect("utf8 valide");
            for hit in &hits {
                let extrait: String = texte.chars().skip(hit.column).take(hit.length).collect();
                assert_eq!(extrait, motif);
            }
        }
    }

    #[test]
    fn apercu_coupe_sur_des_caracteres() {
        let root = workspace("apercu");
        let mut ligne = String::new();
        for _ in 0..400 {
            ligne.push('é');
        }
        ligne.push_str("cible");
        for _ in 0..400 {
            ligne.push('é');
        }
        ligne.push('\n');
        let file = write(&root, "a.txt", ligne.as_bytes());
        let hits = run(&[file], "cible", false, false);
        assert_eq!(hits.len(), 1);
        let apercu = &hits[0].preview;
        assert!(apercu.chars().count() <= PREVIEW_CHARS);
        assert!(apercu.contains("cible"));
        assert!(!apercu.contains('\u{fffd}'));
        assert!(apercu.chars().filter(|&c| c == 'é').count() >= 60);
    }

    #[test]
    fn tabulations_et_retours_chariot_nettoyes() {
        let root = workspace("controle");
        let file = write(&root, "a.rs", b"\tlet x = cible;\r\nautre\n");
        let hits = run(&[file], "cible", false, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 0);
        assert_eq!(hits[0].column, 9);
        assert!(!hits[0].preview.contains('\r'));
        assert!(!hits[0].preview.contains('\t'));
    }

    #[test]
    fn fichiers_binaires_et_volumineux_ignores() {
        let root = workspace("binaire");
        let binaire = write(&root, "b.bin", b"cible\0cible");
        let texte = write(&root, "t.rs", b"cible\n");
        let hits = run(&[binaire, texte], "cible", false, false);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn plafond_de_correspondances() {
        let root = workspace("plafond");
        let mut contenu = String::new();
        for _ in 0..3000 {
            contenu.push_str("cible\n");
        }
        let a = write(&root, "a.rs", contenu.as_bytes());
        let b = write(&root, "b.rs", contenu.as_bytes());
        let c = write(&root, "c.rs", contenu.as_bytes());
        let mut search = ProjectSearch::new();
        search.start(&[a, b, c], "cible", false, false);
        let deadline = Instant::now() + Duration::from_secs(20);
        while search.running() && Instant::now() < deadline {
            search.poll();
        }
        search.poll();
        assert!(!search.running());
        assert_eq!(search.hits().len(), MAX_HITS);
        assert!(search.capped());
    }

    #[test]
    fn ordre_par_fichier_puis_par_ligne() {
        let root = workspace("ordre");
        let mut files = Vec::new();
        for index in 0..24 {
            let mut contenu = String::new();
            for ligne in 0..8 {
                contenu.push_str(&format!("cible {index} {ligne}\n"));
            }
            files.push(write(&root, &format!("f{index:02}.rs"), contenu.as_bytes()));
        }
        let hits = run(&files, "cible", false, false);
        assert_eq!(hits.len(), 24 * 8);
        for (position, hit) in hits.iter().enumerate() {
            let fichier = position / 8;
            let ligne = position % 8;
            assert_eq!(hit.line, ligne);
            assert_eq!(hit.path, files[fichier]);
        }
    }

    #[test]
    fn relance_annule_la_precedente() {
        let root = workspace("relance");
        let mut contenu = String::new();
        for index in 0..40000 {
            contenu.push_str(&format!("alpha {index} beta\n"));
        }
        let mut files = Vec::new();
        for index in 0..12 {
            files.push(write(&root, &format!("f{index}.rs"), contenu.as_bytes()));
        }
        let mut search = ProjectSearch::new();
        let debut = Instant::now();
        search.start(&files, "alpha", false, false);
        assert!(debut.elapsed() < Duration::from_millis(100), "start bloque");
        search.poll();
        let debut = Instant::now();
        search.start(&files, "beta", false, false);
        assert!(debut.elapsed() < Duration::from_millis(100), "relance bloquante");
        assert_eq!(search.needle(), "beta");
        assert!(search.hits().is_empty());
        let deadline = Instant::now() + Duration::from_secs(30);
        while search.running() && Instant::now() < deadline {
            search.poll();
        }
        search.poll();
        assert!(!search.running());
        assert_eq!(search.hits().len(), MAX_HITS);
        for hit in search.hits() {
            assert!(hit.preview.contains("beta"));
        }
        search.cancel();
        let deadline = Instant::now() + Duration::from_secs(30);
        while search.threads() > 0 && Instant::now() < deadline {
            search.poll();
        }
        assert_eq!(search.threads(), 0);
    }

    #[test]
    fn progression_et_annulation() {
        let root = workspace("progression");
        let mut files = Vec::new();
        for index in 0..64 {
            files.push(write(&root, &format!("f{index}.rs"), b"rien du tout\n"));
        }
        let mut search = ProjectSearch::new();
        assert_eq!(search.progress(), (0, 0));
        search.start(&files, "cible", false, false);
        let (_, total) = search.progress();
        assert_eq!(total, 64);
        let deadline = Instant::now() + Duration::from_secs(20);
        while search.running() && Instant::now() < deadline {
            search.poll();
        }
        search.poll();
        assert_eq!(search.progress(), (64, 64));
        assert!(search.hits().is_empty());
        search.start(&files, "", false, false);
        assert!(!search.running());
        assert_eq!(search.progress(), (0, 0));
    }

    fn chemins(root: &Path, entrees: &[&str]) -> Vec<PathBuf> {
        entrees.iter().map(|entree| root.join(entree)).collect()
    }

    #[test]
    fn ouverture_rapide_classe_le_nom_de_fichier() {
        let root = Path::new("/projet");
        let files = chemins(
            root,
            &[
                "src/render/pipeline.rs",
                "src/main.rs",
                "docs/main_notes.md",
                "src/text/mainframe.rs",
                "target/debug/build.rs",
            ],
        );
        let mut finder = FileFinder::new();
        finder.set_files(&files, root);
        let resultats = finder.query("main", 5);
        assert_eq!(resultats.len(), 3);
        assert_eq!(resultats[0].display, "src/main.rs");
        assert!(resultats[0].score > resultats[1].score);
        let court = finder.query("pipe", 3);
        assert_eq!(court.len(), 1);
        assert_eq!(court[0].display, "src/render/pipeline.rs");
        assert_eq!(court[0].path, files[0]);
    }

    #[test]
    fn ouverture_rapide_motif_vide_et_limite() {
        let root = Path::new("/projet");
        let files = chemins(root, &["a.rs", "b.rs", "c.rs", "d.rs"]);
        let mut finder = FileFinder::new();
        finder.set_files(&files, root);
        assert_eq!(finder.query("", 2).len(), 2);
        assert_eq!(finder.query("", 99).len(), 4);
        assert_eq!(finder.query("rs", 3).len(), 3);
        assert_eq!(finder.query("zzzz", 10).len(), 0);
        assert_eq!(finder.query("a", 0).len(), 0);
    }

    #[test]
    fn ouverture_rapide_accents_et_motifs_composes() {
        let root = Path::new("/projet");
        let files = chemins(
            root,
            &[
                "src/données/café.rs",
                "src/data/coffee.rs",
                "src/données/thé.rs",
            ],
        );
        let mut finder = FileFinder::new();
        finder.set_files(&files, root);
        let accent = finder.query("café", 5);
        assert_eq!(accent.len(), 1);
        assert_eq!(accent[0].display, "src/données/café.rs");
        let sans_accent = finder.query("cafe", 5);
        assert_eq!(sans_accent[0].display, "src/données/café.rs");
        let compose = finder.query("data cof", 5);
        assert_eq!(compose.len(), 1);
        assert_eq!(compose[0].display, "src/data/coffee.rs");
        let exact = finder.query("'thé", 5);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].display, "src/données/thé.rs");
    }

    #[test]
    fn ouverture_rapide_accents_dans_un_grand_index() {
        let root = Path::new("/depot");
        let mut files = Vec::with_capacity(20_001);
        for index in 0..20_000usize {
            files.push(root.join(format!("src/module/fichier_{index}.rs")));
        }
        files.push(root.join("src/données/relevé.rs"));
        let mut finder = FileFinder::new();
        finder.set_files(&files, root);
        let resultats = finder.query("releve", 10);
        assert_eq!(resultats[0].display, "src/données/relevé.rs");
        let direct = finder.query("relevé", 10);
        assert_eq!(direct[0].display, "src/données/relevé.rs");
    }

    #[test]
    fn ouverture_rapide_grand_index() {
        let mots = [
            "engine", "render", "shader", "core", "util", "mesh", "audio", "input", "scene",
            "math", "gpu", "texture", "buffer", "pipeline", "widget",
        ];
        let root = Path::new("/depot");
        let mut files = Vec::with_capacity(50_000);
        for index in 0..50_000usize {
            let a = mots[index % mots.len()];
            let b = mots[(index / 7) % mots.len()];
            let c = mots[(index / 53) % mots.len()];
            files.push(root.join(format!("crates/{a}_{b}/src/{c}/module_{index}.rs")));
        }
        let mut finder = FileFinder::new();
        finder.set_files(&files, root);
        assert_eq!(finder.len(), 50_000);
        let mut pire = Duration::ZERO;
        for motif in ["s", "sr", "src", "ren", "mod", "zzz"] {
            let debut = Instant::now();
            let resultats = finder.query(motif, 50);
            let ecoule = debut.elapsed();
            if ecoule > pire {
                pire = ecoule;
            }
            if motif == "zzz" {
                assert!(resultats.is_empty());
            } else {
                assert_eq!(resultats.len(), 50);
            }
        }
        assert!(pire < Duration::from_millis(60), "trop lent: {pire:?}");
        let cible = finder.query("module_49999", 5);
        assert_eq!(
            cible[0].display,
            "crates/util_shader/src/pipeline/module_49999.rs"
        );
    }
}
