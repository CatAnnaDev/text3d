use std::collections::HashSet;

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::syntax::{Highlighter, Language, SymbolKind};
use crate::text::TextBuffer;

pub const MIN_PREFIX: usize = 2;
pub const VISIBLE_ROWS: usize = 8;
const MAX_ITEMS: usize = 48;
const MAX_WORDS: usize = 4096;
const MIN_WORD: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    Type,
    Function,
    Method,
    Class,
    Interface,
    Module,
    Macro,
    Word,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Keyword => "kw",
            Kind::Type => "ty",
            Kind::Function => "fn",
            Kind::Method => "me",
            Kind::Class => "cl",
            Kind::Interface => "tr",
            Kind::Module => "mod",
            Kind::Macro => "ma",
            Kind::Word => "..",
        }
    }

    pub fn style(self) -> u8 {
        match self {
            Kind::Keyword => 1,
            Kind::Type | Kind::Class | Kind::Interface => 2,
            Kind::Function => 3,
            Kind::Method => 4,
            Kind::Macro => 5,
            Kind::Module => 15,
            Kind::Word => 0,
        }
    }

    fn bonus(self) -> u32 {
        match self {
            Kind::Word => 0,
            Kind::Keyword | Kind::Type => 40,
            _ => 90,
        }
    }

    fn of(symbol: SymbolKind) -> Kind {
        match symbol {
            SymbolKind::Function => Kind::Function,
            SymbolKind::Method => Kind::Method,
            SymbolKind::Class => Kind::Class,
            SymbolKind::Interface => Kind::Interface,
            SymbolKind::Module => Kind::Module,
            SymbolKind::Macro => Kind::Macro,
            SymbolKind::Other => Kind::Word,
        }
    }
}

pub struct Candidate {
    pub text: String,
    pub kind: Kind,
}

pub struct Completion {
    pub active: bool,
    pub items: Vec<Candidate>,
    pub selected: usize,
    pub scroll: usize,
    pub prefix_chars: usize,
    prefix: String,
    pool: Vec<Candidate>,
    pool_version: u64,
    pool_language: Option<Language>,
    matcher: Matcher,
    haystack: Vec<char>,
    ranked: Vec<(u32, usize)>,
}

impl Completion {
    pub fn new() -> Completion {
        Completion {
            active: false,
            items: Vec::new(),
            selected: 0,
            scroll: 0,
            prefix_chars: 0,
            prefix: String::new(),
            pool: Vec::new(),
            pool_version: 0,
            pool_language: None,
            matcher: Matcher::new(Config::DEFAULT),
            haystack: Vec::new(),
            ranked: Vec::new(),
        }
    }

    pub fn dismiss(&mut self) {
        self.active = false;
        self.items.clear();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let count = self.items.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(count);
        self.selected = next as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    pub fn selection(&self) -> Option<&Candidate> {
        self.items.get(self.selected)
    }

    pub fn refresh(
        &mut self,
        text: &TextBuffer,
        language: Option<Language>,
        highlighter: Option<&Highlighter>,
        min_prefix: usize,
    ) {
        let prefix = text.word_prefix();
        if prefix.chars().count() < min_prefix {
            self.dismiss();
            return;
        }
        self.prefix.clear();
        self.prefix.push_str(prefix);
        self.prefix_chars = self.prefix.chars().count();

        if self.pool_version != text.version() || self.pool_language != language {
            self.pool_version = text.version();
            self.pool_language = language;
            self.build_pool(text, language, highlighter);
        }

        let atom = Atom::new(
            &self.prefix,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        );

        self.ranked.clear();
        for (index, candidate) in self.pool.iter().enumerate() {
            if candidate.text == self.prefix {
                continue;
            }
            let haystack = Utf32Str::new(&candidate.text, &mut self.haystack);
            if let Some(score) = atom.score(haystack, &mut self.matcher) {
                self.ranked.push((u32::from(score) + candidate.kind.bonus(), index));
            }
        }
        self.ranked
            .sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.ranked.truncate(MAX_ITEMS);

        self.items.clear();
        for &(_, index) in &self.ranked {
            let candidate = &self.pool[index];
            self.items.push(Candidate {
                text: candidate.text.clone(),
                kind: candidate.kind,
            });
        }
        self.selected = 0;
        self.scroll = 0;
        self.active = !self.items.is_empty();
    }

    fn build_pool(
        &mut self,
        text: &TextBuffer,
        language: Option<Language>,
        highlighter: Option<&Highlighter>,
    ) {
        self.pool.clear();
        let mut seen: HashSet<&str> = HashSet::new();
        let source = text.source();

        let mut symbols: Vec<(String, SymbolKind)> = Vec::new();
        if let Some(highlighter) = highlighter {
            highlighter.collect_symbols(source, &mut symbols);
        }
        for (name, kind) in &symbols {
            if seen.insert(name.as_str()) {
                self.pool.push(Candidate {
                    text: name.clone(),
                    kind: Kind::of(*kind),
                });
            }
        }

        if let Some(language) = language {
            let (keywords, types) = match language {
                Language::Rust => (RUST_KEYWORDS, RUST_TYPES),
                Language::CSharp => (CSHARP_KEYWORDS, CSHARP_TYPES),
            };
            for (words, kind) in [(keywords, Kind::Keyword), (types, Kind::Type)] {
                for word in words {
                    if seen.insert(word) {
                        self.pool.push(Candidate { text: (*word).to_string(), kind });
                    }
                }
            }
        }

        let bytes = source.as_bytes();
        let mut start = 0;
        let mut words = 0;
        while start < bytes.len() && words < MAX_WORDS {
            if !is_word_byte(bytes[start]) {
                start += 1;
                continue;
            }
            let mut end = start;
            while end < bytes.len() && is_word_byte(bytes[end]) {
                end += 1;
            }
            let word = &source[start..end];
            if word.chars().count() >= MIN_WORD
                && word.chars().next().is_some_and(|c| !c.is_ascii_digit())
                && seen.insert(word)
            {
                self.pool.push(Candidate { text: word.to_string(), kind: Kind::Word });
                words += 1;
            }
            start = end;
        }
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || byte >= 0x80
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "union",
    "unsafe", "use", "where", "while", "assert!", "assert_eq!", "dbg!", "format!", "matches!",
    "panic!", "println!", "eprintln!", "todo!", "unimplemented!", "unreachable!", "vec!", "write!",
    "writeln!", "macro_rules!",
];

const RUST_TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16",
    "u32", "u64", "u128", "usize", "String", "Vec", "VecDeque", "Option", "Some", "None", "Result",
    "Ok", "Err", "Box", "Rc", "Arc", "Cell", "RefCell", "Mutex", "RwLock", "HashMap", "HashSet",
    "BTreeMap", "BTreeSet", "Cow", "Path", "PathBuf", "Iterator", "IntoIterator", "Default",
    "Clone", "Copy", "Debug", "Display", "From", "Into", "TryFrom", "TryInto", "PartialEq", "Eq",
    "Hash", "Ord", "PartialOrd", "Send", "Sync", "Sized", "Drop", "Fn", "FnMut", "FnOnce", "Range",
];

const CSHARP_KEYWORDS: &[&str] = &[
    "abstract", "as", "base", "break", "case", "catch", "checked", "class", "const", "continue",
    "default", "delegate", "do", "else", "enum", "event", "explicit", "extern", "false", "finally",
    "fixed", "for", "foreach", "goto", "if", "implicit", "in", "interface", "internal", "is",
    "lock", "namespace", "new", "null", "operator", "out", "override", "params", "private",
    "protected", "public", "readonly", "ref", "return", "sealed", "sizeof", "stackalloc", "static",
    "struct", "switch", "this", "throw", "true", "try", "typeof", "unchecked", "unsafe", "using",
    "virtual", "volatile", "while", "add", "and", "async", "await", "descending", "dynamic",
    "equals", "from", "get", "global", "group", "init", "into", "join", "let", "nameof", "not",
    "on", "or", "orderby", "partial", "record", "remove", "required", "select", "set", "value",
    "var", "when", "where", "with", "yield", "ascending", "by",
];

const CSHARP_TYPES: &[&str] = &[
    "bool", "byte", "char", "decimal", "double", "float", "int", "long", "nint", "nuint", "object",
    "sbyte", "short", "string", "uint", "ulong", "ushort", "void", "Action", "Array", "Boolean",
    "Console", "Convert", "DateTime", "Dictionary", "Double", "Enumerable", "Exception", "Func",
    "Guid", "HashSet", "IAsyncEnumerable", "ICollection", "IDictionary", "IDisposable",
    "IEnumerable", "IEnumerator", "IList", "IReadOnlyList", "Int32", "Int64", "KeyValuePair",
    "LinkedList", "List", "Math", "Nullable", "Object", "Predicate", "Queue", "Random", "Regex",
    "Span", "Stack", "Stream", "String", "StringBuilder", "Task", "TimeSpan", "Tuple", "Type",
    "Uri", "ValueTask",
];
