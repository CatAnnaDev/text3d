use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::syntax::{Highlighter, Language};
use crate::text::{Cursor, TextBuffer};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const PROBE_BYTES: usize = 8192;
const JUMP_LIMIT: usize = 128;
const GRAMMARS: usize = 2;
const UNTITLED: &str = "sans titre";

pub struct Tab {
    pub path: Option<PathBuf>,
    pub name: String,
    pub modified: bool,
    pub language: Option<Language>,
}

struct Document {
    id: u64,
    buffer: TextBuffer,
    language: Option<Language>,
    version: i32,
    cursor: Cursor,
    scroll: (f32, f32),
    shaded_version: u64,
    shaded_lines: Range<usize>,
}

impl Document {
    fn new(buffer: TextBuffer, id: u64) -> Document {
        let language = Language::detect(buffer.path.as_deref());
        let cursor = buffer.cursor();
        Document {
            id,
            buffer,
            language,
            version: 1,
            cursor,
            scroll: (0.0, 0.0),
            shaded_version: 0,
            shaded_lines: 0..0,
        }
    }

    fn is_scratch(&self) -> bool {
        self.buffer.path.is_none()
            && !self.buffer.modified
            && self.buffer.line_count() == 1
            && self.buffer.lines[0].is_empty()
    }
}

struct Jump {
    path: PathBuf,
    cursor: Cursor,
}

struct Shading {
    highlighter: Option<Highlighter>,
    owner: u64,
    tried: bool,
}

impl Shading {
    fn idle() -> Shading {
        Shading { highlighter: None, owner: 0, tried: false }
    }
}

pub struct Workspace {
    docs: Vec<Document>,
    tabs: Vec<Tab>,
    active: usize,
    jumps: Vec<Jump>,
    jump_at: usize,
    shading: [Shading; GRAMMARS],
    next_id: u64,
}

impl Workspace {
    pub fn new(buffer: TextBuffer) -> Workspace {
        let mut workspace = Workspace {
            docs: vec![Document::new(buffer, 1)],
            tabs: Vec::new(),
            active: 0,
            jumps: Vec::new(),
            jump_at: 0,
            shading: [Shading::idle(), Shading::idle()],
            next_id: 1,
        };
        workspace.refresh_tabs();
        workspace
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn buffer(&self) -> &TextBuffer {
        &self.docs[self.active].buffer
    }

    pub fn buffer_mut(&mut self) -> &mut TextBuffer {
        &mut self.docs[self.active].buffer
    }

    pub fn buffer_at(&self, index: usize) -> &TextBuffer {
        &self.docs[index.min(self.docs.len() - 1)].buffer
    }

    pub fn language(&self) -> Option<Language> {
        self.docs[self.active].language
    }

    pub fn highlighter(&self) -> Option<&Highlighter> {
        let doc = &self.docs[self.active];
        let shading = &self.shading[slot_of(doc.language?)];
        if shading.owner != doc.id {
            return None;
        }
        shading.highlighter.as_ref()
    }

    pub fn highlighter_mut(&mut self) -> Option<&mut Highlighter> {
        let doc = &self.docs[self.active];
        let shading = &mut self.shading[slot_of(doc.language?)];
        if shading.owner != doc.id {
            return None;
        }
        shading.highlighter.as_mut()
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn scroll(&self) -> (f32, f32) {
        self.docs[self.active].scroll
    }

    pub fn set_scroll(&mut self, scroll: (f32, f32)) {
        self.docs[self.active].scroll = scroll;
    }

    pub fn open(&mut self, path: &Path) -> Result<usize, String> {
        let full = canonical(path)?;
        if let Some(index) = self.index_of(&full) {
            self.activate(index);
            return Ok(index);
        }
        let text = read_source(&full)?;
        let buffer = TextBuffer::from_str(&text, Some(full));
        let doc = Document::new(buffer, self.claim_id());
        let index = if self.docs.len() == 1 && self.docs[0].is_scratch() {
            self.docs[0] = doc;
            0
        } else {
            self.store_cursor();
            self.docs.push(doc);
            self.docs.len() - 1
        };
        self.active = index;
        self.refresh_tabs();
        self.restore_cursor();
        Ok(index)
    }

    pub fn activate(&mut self, index: usize) {
        if index >= self.docs.len() || index == self.active {
            return;
        }
        self.store_cursor();
        self.active = index;
        self.restore_cursor();
        self.refresh();
    }

    pub fn close(&mut self, index: usize) -> bool {
        if index >= self.docs.len() || self.docs[index].buffer.modified {
            return false;
        }
        if self.docs.len() == 1 {
            if self.docs[0].is_scratch() {
                return true;
            }
            self.docs[0] = Document::new(TextBuffer::from_str("", None), self.claim_id());
            self.active = 0;
            self.refresh_tabs();
            return true;
        }
        self.docs.remove(index);
        if self.active > index || self.active == self.docs.len() {
            self.active -= 1;
        }
        self.refresh_tabs();
        self.restore_cursor();
        true
    }

    pub fn cycle(&mut self, forward: bool) {
        let count = self.docs.len();
        if count < 2 {
            return;
        }
        let next = if forward {
            (self.active + 1) % count
        } else {
            (self.active + count - 1) % count
        };
        self.activate(next);
    }

    pub fn save_active(&mut self) {
        self.docs[self.active].buffer.save();
        self.after_save();
    }

    pub fn save_all(&mut self) -> usize {
        let mut saved = 0;
        for doc in self.docs.iter_mut() {
            if !doc.buffer.modified {
                continue;
            }
            doc.buffer.save();
            if !doc.buffer.modified {
                saved += 1;
            }
        }
        self.after_save();
        saved
    }

    pub fn refresh(&mut self) {
        for (tab, doc) in self.tabs.iter_mut().zip(self.docs.iter()) {
            tab.modified = doc.buffer.modified;
        }
    }

    pub fn dirty(&self) -> usize {
        self.docs.iter().filter(|doc| doc.buffer.modified).count()
    }

    pub fn find(&self, path: &Path) -> Option<usize> {
        match std::fs::canonicalize(path) {
            Ok(full) => self.index_of(&full),
            Err(_) => self.index_of(path),
        }
    }

    pub fn goto(&mut self, path: &Path, cursor: Cursor) -> Result<(), String> {
        self.open(path)?;
        let doc = &mut self.docs[self.active];
        doc.buffer.set_cursor(cursor, false);
        doc.cursor = doc.buffer.cursor();
        Ok(())
    }

    pub fn push_jump(&mut self) {
        let doc = &self.docs[self.active];
        let Some(path) = doc.buffer.path.as_ref() else {
            return;
        };
        let cursor = doc.buffer.cursor();
        self.jumps.truncate(self.jump_at);
        if let Some(last) = self.jumps.last_mut()
            && last.path == *path
            && last.cursor.line == cursor.line
        {
            last.cursor = cursor;
            self.jump_at = self.jumps.len();
            return;
        }
        let path = path.clone();
        self.jumps.push(Jump { path, cursor });
        if self.jumps.len() > JUMP_LIMIT {
            self.jumps.remove(0);
        }
        self.jump_at = self.jumps.len();
    }

    pub fn back(&mut self) -> Option<(PathBuf, Cursor)> {
        if self.jump_at == 0 {
            return None;
        }
        if self.jump_at == self.jumps.len() {
            let doc = &self.docs[self.active];
            let here = doc
                .buffer
                .path
                .as_ref()
                .map(|path| Jump { path: path.clone(), cursor: doc.buffer.cursor() });
            if let Some(here) = here {
                self.jumps.push(here);
            }
        }
        self.jump_at -= 1;
        let jump = &self.jumps[self.jump_at];
        Some((jump.path.clone(), jump.cursor))
    }

    pub fn forward(&mut self) -> Option<(PathBuf, Cursor)> {
        if self.jump_at + 1 >= self.jumps.len() {
            return None;
        }
        self.jump_at += 1;
        let jump = &self.jumps[self.jump_at];
        Some((jump.path.clone(), jump.cursor))
    }

    pub fn doc_version(&self, index: usize) -> i32 {
        self.docs.get(index).map(|doc| doc.version).unwrap_or(0)
    }

    pub fn bump_version(&mut self, index: usize) -> i32 {
        match self.docs.get_mut(index) {
            Some(doc) => {
                doc.version = doc.version.wrapping_add(1);
                doc.version
            }
            None => 0,
        }
    }

    pub fn sync_highlighter(&mut self, lines: Range<usize>) {
        self.refresh();
        let doc = &mut self.docs[self.active];
        doc.buffer.sync();
        let Some(language) = doc.language else {
            return;
        };
        let shading = &mut self.shading[slot_of(language)];
        if !shading.tried {
            shading.tried = true;
            shading.highlighter = Highlighter::new(language);
        }
        let Some(highlighter) = shading.highlighter.as_mut() else {
            return;
        };
        let version = doc.buffer.version();
        if shading.owner == doc.id && doc.shaded_version == version && doc.shaded_lines == lines {
            return;
        }
        shading.owner = doc.id;
        doc.shaded_version = version;
        doc.shaded_lines = lines.clone();
        let window = doc.buffer.byte_range(lines);
        highlighter.update(doc.buffer.source(), doc.buffer.line_starts(), window);
    }

    fn after_save(&mut self) {
        for doc in self.docs.iter_mut() {
            if doc.language.is_none() {
                doc.language = Language::detect(doc.buffer.path.as_deref());
            }
        }
        self.refresh_tabs();
    }

    fn claim_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.docs
            .iter()
            .position(|doc| doc.buffer.path.as_deref() == Some(path))
    }

    fn store_cursor(&mut self) {
        let doc = &mut self.docs[self.active];
        doc.cursor = doc.buffer.cursor();
    }

    fn restore_cursor(&mut self) {
        let doc = &mut self.docs[self.active];
        let cursor = doc.cursor;
        doc.buffer.set_cursor(cursor, false);
        doc.cursor = doc.buffer.cursor();
    }

    fn refresh_tabs(&mut self) {
        if self.tabs.len() != self.docs.len() {
            self.tabs.resize_with(self.docs.len(), || Tab {
                path: None,
                name: String::new(),
                modified: false,
                language: None,
            });
        }
        let docs = &self.docs;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let doc = &docs[index];
            let path = doc.buffer.path.as_deref();
            tab.name.clear();
            match path {
                Some(path) => {
                    let base = base_name(path);
                    let shared = docs.iter().enumerate().any(|(other, doc)| {
                        other != index
                            && doc.buffer.path.as_deref().map(base_name) == Some(base)
                    });
                    if shared
                        && let Some(parent) = path.parent().and_then(base_of)
                    {
                        tab.name.push_str(parent);
                        tab.name.push('/');
                    }
                    tab.name.push_str(base);
                }
                None => tab.name.push_str(UNTITLED),
            }
            if tab.path.as_deref() != path {
                tab.path = path.map(Path::to_path_buf);
            }
            tab.language = doc.language;
            tab.modified = doc.buffer.modified;
        }
    }
}

fn slot_of(language: Language) -> usize {
    match language {
        Language::Rust => 0,
        Language::CSharp => 1,
    }
}

fn base_of(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn base_name(path: &Path) -> &str {
    base_of(path).unwrap_or(UNTITLED)
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|err| format!("chemin introuvable {}: {err}", path.display()))
}

fn read_source(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| format!("lecture impossible: {err}"))?;
    let meta = file
        .metadata()
        .map_err(|err| format!("lecture impossible: {err}"))?;
    if meta.is_dir() {
        return Err(format!("{} est un dossier", path.display()));
    }
    let size = meta.len();
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "fichier trop volumineux: {} mo (limite {} mo)",
            size / (1024 * 1024),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let mut probe = [0u8; PROBE_BYTES];
    let mut filled = 0;
    while filled < PROBE_BYTES {
        match file.read(&mut probe[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("lecture impossible: {err}")),
        }
    }
    if probe[..filled].contains(&0) {
        return Err(format!("fichier binaire refuse: {}", path.display()));
    }
    let mut bytes = Vec::with_capacity(size as usize + 1);
    bytes.extend_from_slice(&probe[..filled]);
    file.read_to_end(&mut bytes)
        .map_err(|err| format!("lecture impossible: {err}"))?;
    String::from_utf8(bytes).map_err(|_| format!("encodage non utf-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::STYLE_TEXT;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("text3d-ws-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        root
    }

    fn file(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, body).expect("ecriture");
        std::fs::canonicalize(&path).expect("canonisation")
    }

    fn empty() -> Workspace {
        Workspace::new(TextBuffer::from_str("", None))
    }

    #[test]
    fn scratch_tab_is_replaced_then_tabs_accumulate() {
        let root = sandbox("accumule");
        let first = file(&root, "un.rs", "fn un() {}\n");
        let second = file(&root, "deux.rs", "fn deux() {}\n");
        let mut workspace = empty();
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace.tabs()[0].name, UNTITLED);
        assert_eq!(workspace.open(&first), Ok(0));
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace.open(&second), Ok(1));
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.active(), 1);
        assert_eq!(workspace.tabs()[0].name, "un.rs");
        assert_eq!(workspace.tabs()[1].name, "deux.rs");
        assert_eq!(workspace.open(&first), Ok(0));
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.active(), 0);
        assert_eq!(workspace.find(&first), Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_and_scroll_come_back_on_activation() {
        let root = sandbox("curseur");
        let first = file(&root, "un.rs", "a\nb\nc\nd\ne\nf\n");
        let second = file(&root, "deux.rs", "1\n2\n3\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.buffer_mut().set_cursor(Cursor { line: 4, column: 1 }, false);
        workspace.set_scroll((3.5, -2.0));
        workspace.open(&second).expect("deux");
        workspace.buffer_mut().set_cursor(Cursor { line: 2, column: 0 }, false);
        assert_eq!(workspace.buffer().cursor(), Cursor { line: 2, column: 0 });
        workspace.activate(0);
        assert_eq!(workspace.buffer().cursor(), Cursor { line: 4, column: 1 });
        assert_eq!(workspace.scroll(), (3.5, -2.0));
        workspace.cycle(true);
        assert_eq!(workspace.active(), 1);
        assert_eq!(workspace.buffer().cursor(), Cursor { line: 2, column: 0 });
        workspace.cycle(false);
        assert_eq!(workspace.active(), 0);
        assert_eq!(workspace.buffer().cursor(), Cursor { line: 4, column: 1 });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn modified_tab_refuses_to_close() {
        let root = sandbox("modifie");
        let first = file(&root, "un.rs", "fn un() {}\n");
        let second = file(&root, "deux.rs", "fn deux() {}\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.open(&second).expect("deux");
        workspace.activate(0);
        workspace.buffer_mut().insert_char('x');
        assert_eq!(workspace.dirty(), 1);
        workspace.sync_highlighter(0..1);
        assert!(workspace.tabs()[0].modified);
        assert!(!workspace.tabs()[1].modified);
        assert!(!workspace.close(0));
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.save_all(), 1);
        assert_eq!(workspace.dirty(), 0);
        assert!(!workspace.tabs()[0].modified);
        workspace.buffer_mut().insert_char('y');
        workspace.refresh();
        assert!(workspace.tabs()[0].modified);
        workspace.save_active();
        assert!(!workspace.tabs()[0].modified);
        assert_eq!(workspace.dirty(), 0);
        assert!(workspace.close(0));
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace.active(), 0);
        assert_eq!(workspace.tabs()[0].name, "deux.rs");
        assert!(workspace.close(0));
        assert_eq!(workspace.len(), 1);
        assert!(workspace.buffer().path.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn binary_and_oversize_files_are_refused() {
        let root = sandbox("refus");
        let binary = file(&root, "bin.dat", "abc\0def");
        let mut workspace = empty();
        let refused = workspace.open(&binary).expect_err("binaire");
        assert!(refused.starts_with("fichier binaire refuse"), "{refused}");
        assert_eq!(workspace.len(), 1);

        let missing = workspace.open(&root.join("absent.rs")).expect_err("absent");
        assert!(missing.starts_with("chemin introuvable"), "{missing}");

        let folder = workspace.open(&root).expect_err("dossier");
        assert!(folder.ends_with("est un dossier"), "{folder}");

        let huge = root.join("gros.txt");
        let chunk = vec![b'a'; 1024 * 1024];
        {
            use std::io::Write;
            let mut out = std::fs::File::create(&huge).expect("gros");
            for _ in 0..33 {
                out.write_all(&chunk).expect("bloc");
            }
        }
        let refused = workspace.open(&huge).expect_err("volume");
        assert_eq!(refused, "fichier trop volumineux: 33 mo (limite 32 mo)");

        let latin = root.join("latin.txt");
        std::fs::write(&latin, [b'e', 0xE9, b'a']).expect("latin");
        let refused = workspace.open(&latin).expect_err("encodage");
        assert!(refused.starts_with("encodage non utf-8"), "{refused}");
        assert_eq!(workspace.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jump_stack_walks_back_and_forward() {
        let root = sandbox("sauts");
        let first = file(&root, "un.rs", "a\nb\nc\nd\ne\nf\ng\nh\n");
        let second = file(&root, "deux.rs", "1\n2\n3\n4\n5\n6\n");
        let third = file(&root, "trois.rs", "x\ny\nz\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        assert!(workspace.back().is_none());

        workspace.buffer_mut().set_cursor(Cursor { line: 2, column: 0 }, false);
        workspace.push_jump();
        workspace.goto(&second, Cursor { line: 4, column: 1 }).expect("saut");
        assert_eq!(workspace.active(), 1);

        let (path, cursor) = workspace.back().expect("retour");
        assert_eq!(path, first);
        assert_eq!(cursor, Cursor { line: 2, column: 0 });
        workspace.goto(&path, cursor).expect("retour applique");
        assert_eq!(workspace.active(), 0);

        let (path, cursor) = workspace.forward().expect("avant");
        assert_eq!(path, second);
        assert_eq!(cursor, Cursor { line: 4, column: 1 });
        workspace.goto(&path, cursor).expect("avant applique");
        assert!(workspace.forward().is_none());

        let (path, _) = workspace.back().expect("retour bis");
        assert_eq!(path, first);
        workspace.goto(&path, cursor).expect("retour bis applique");
        workspace.push_jump();
        workspace.goto(&third, Cursor { line: 1, column: 0 }).expect("nouvelle branche");
        assert!(workspace.forward().is_none());
        let (path, _) = workspace.back().expect("retour ter");
        assert_eq!(path, first);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jump_stack_reopens_a_closed_file() {
        let root = sandbox("reouvre");
        let first = file(&root, "un.rs", "a\nb\nc\n");
        let second = file(&root, "deux.rs", "1\n2\n3\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.buffer_mut().set_cursor(Cursor { line: 1, column: 0 }, false);
        workspace.push_jump();
        workspace.goto(&second, Cursor { line: 2, column: 0 }).expect("saut");
        assert!(workspace.close(0));
        assert_eq!(workspace.len(), 1);
        let (path, cursor) = workspace.back().expect("retour");
        workspace.goto(&path, cursor).expect("reouverture");
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace.buffer().path.as_deref(), Some(first.as_path()));
        assert_eq!(workspace.buffer().cursor(), Cursor { line: 1, column: 0 });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_line_jumps_are_coalesced() {
        let root = sandbox("coalesce");
        let first = file(&root, "un.rs", "aaaa\nbbbb\ncccc\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.buffer_mut().set_cursor(Cursor { line: 1, column: 0 }, false);
        workspace.push_jump();
        workspace.buffer_mut().set_cursor(Cursor { line: 1, column: 3 }, false);
        workspace.push_jump();
        assert_eq!(workspace.jumps.len(), 1);
        assert_eq!(workspace.jumps[0].cursor, Cursor { line: 1, column: 3 });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_tab_keeps_its_language_and_highlighter() {
        let root = sandbox("langages");
        let rust = file(&root, "un.rs", "fn main() { let value = 1; }\n");
        let sharp = file(&root, "Deux.cs", "class Deux { void Trois() { } }\n");
        let plain = file(&root, "notes.txt", "juste du texte\n");
        let mut workspace = empty();
        workspace.open(&rust).expect("rust");
        workspace.open(&sharp).expect("csharp");
        workspace.open(&plain).expect("texte");

        assert!(workspace.tabs()[0].language == Some(Language::Rust));
        assert!(workspace.tabs()[1].language == Some(Language::CSharp));
        assert!(workspace.tabs()[2].language.is_none());

        workspace.sync_highlighter(0..1);
        assert!(workspace.highlighter().is_none());

        workspace.activate(0);
        workspace.sync_highlighter(0..1);
        let rust_style = workspace.highlighter().expect("rust colore").style_at(0);
        assert_ne!(rust_style, STYLE_TEXT);

        workspace.activate(1);
        workspace.sync_highlighter(0..1);
        let sharp_style = workspace.highlighter().expect("csharp colore").style_at(0);
        assert_eq!(sharp_style, rust_style);
        let sharp_text = workspace.highlighter().expect("csharp colore").style_at(6);
        assert_ne!(sharp_text, rust_style);

        workspace.activate(0);
        workspace.sync_highlighter(0..1);
        assert_eq!(workspace.highlighter().expect("rust colore").style_at(0), rust_style);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shared_grammar_never_colors_the_wrong_tab() {
        let root = sandbox("grammaire");
        let first = file(&root, "un.rs", "fn un() { let a = 1; }\n");
        let second = file(&root, "deux.rs", "struct Deux { champ: u32 }\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.sync_highlighter(0..1);
        assert!(workspace.highlighter().is_some());
        workspace.open(&second).expect("deux");
        assert!(workspace.highlighter().is_none());
        workspace.sync_highlighter(0..1);
        let style = workspace.highlighter().expect("deux colore").style_at(0);
        assert_ne!(style, STYLE_TEXT);
        workspace.activate(0);
        assert!(workspace.highlighter().is_none());
        workspace.sync_highlighter(0..1);
        assert!(workspace.highlighter().is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn twin_names_show_their_folder() {
        let root = sandbox("homonymes");
        std::fs::create_dir_all(root.join("a")).expect("dossier a");
        std::fs::create_dir_all(root.join("b")).expect("dossier b");
        let first = file(&root, "a/mod.rs", "fn a() {}\n");
        let second = file(&root, "b/mod.rs", "fn b() {}\n");
        let mut workspace = empty();
        workspace.open(&first).expect("a");
        assert_eq!(workspace.tabs()[0].name, "mod.rs");
        workspace.open(&second).expect("b");
        assert_eq!(workspace.tabs()[0].name, "a/mod.rs");
        assert_eq!(workspace.tabs()[1].name, "b/mod.rs");
        assert!(workspace.close(1));
        assert_eq!(workspace.tabs()[0].name, "mod.rs");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn document_versions_are_independent() {
        let root = sandbox("versions");
        let first = file(&root, "un.rs", "fn un() {}\n");
        let second = file(&root, "deux.rs", "fn deux() {}\n");
        let mut workspace = empty();
        workspace.open(&first).expect("un");
        workspace.open(&second).expect("deux");
        assert_eq!(workspace.doc_version(0), 1);
        assert_eq!(workspace.doc_version(1), 1);
        assert_eq!(workspace.bump_version(1), 2);
        assert_eq!(workspace.bump_version(1), 3);
        assert_eq!(workspace.doc_version(0), 1);
        assert_eq!(workspace.doc_version(9), 0);
        assert_eq!(workspace.bump_version(9), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn accessors_stay_in_range() {
        let mut workspace = empty();
        workspace.activate(7);
        assert_eq!(workspace.active(), 0);
        assert_eq!(workspace.buffer_at(7).line_count(), 1);
        assert!(!workspace.close(7));
        workspace.cycle(true);
        assert_eq!(workspace.active(), 0);
        assert!(workspace.back().is_none());
        assert!(workspace.forward().is_none());
        workspace.push_jump();
        assert!(workspace.back().is_none());
    }
}
