use std::collections::HashSet;

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::lsp::protocol::CompletionItem;
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

    pub fn from_language(code: u32) -> Kind {
        match code {
            2 | 4 | 23 => Kind::Method,
            3 => Kind::Function,
            7 => Kind::Class,
            8 => Kind::Interface,
            9 | 19 => Kind::Module,
            13 | 22 | 25 => Kind::Type,
            14 | 24 => Kind::Keyword,
            15 => Kind::Macro,
            _ => Kind::Word,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Local,
    Language,
}

pub struct Candidate {
    pub text: String,
    pub detail: String,
    pub kind: Kind,
}

pub struct Completion {
    pub active: bool,
    pub items: Vec<Candidate>,
    pub selected: usize,
    pub scroll: usize,
    pub prefix_chars: usize,
    prefix: String,
    source: Source,
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
            source: Source::Local,
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
        self.source = Source::Local;
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
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
        self.source = Source::Local;

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
        self.items.reserve(self.ranked.len());
        for &(_, index) in &self.ranked {
            let candidate = &self.pool[index];
            self.items.push(Candidate {
                text: candidate.text.clone(),
                detail: String::new(),
                kind: candidate.kind,
            });
        }
        self.selected = 0;
        self.scroll = 0;
        self.active = !self.items.is_empty();
    }

    pub fn set_language_items(&mut self, items: &[CompletionItem], prefix: &str) {
        if items.is_empty() {
            return;
        }
        self.prefix.clear();
        self.prefix.push_str(prefix);
        self.prefix_chars = self.prefix.chars().count();

        self.ranked.clear();
        self.ranked.reserve(items.len());
        if self.prefix.is_empty() {
            for index in 0..items.len() {
                self.ranked.push((0, index));
            }
        } else {
            let atom = Atom::new(
                &self.prefix,
                CaseMatching::Smart,
                Normalization::Smart,
                AtomKind::Fuzzy,
                false,
            );
            for (index, item) in items.iter().enumerate() {
                let subject = if item.filter.is_empty() { &item.label } else { &item.filter };
                let haystack = Utf32Str::new(subject, &mut self.haystack);
                if let Some(score) = atom.score(haystack, &mut self.matcher) {
                    let kind = Kind::from_language(item.kind);
                    self.ranked.push((u32::from(score) + kind.bonus(), index));
                }
            }
        }
        if self.ranked.is_empty() {
            return;
        }
        self.ranked.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| items[a.1].sort.cmp(&items[b.1].sort))
                .then(a.1.cmp(&b.1))
        });
        self.ranked.truncate(MAX_ITEMS);

        self.items.clear();
        self.items.reserve(self.ranked.len());
        for &(_, index) in &self.ranked {
            let item = &items[index];
            self.items.push(Candidate {
                text: item.insert.clone(),
                detail: item.detail.clone(),
                kind: Kind::from_language(item.kind),
            });
        }
        self.selected = 0;
        self.scroll = 0;
        self.source = Source::Language;
        self.active = true;
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
                    detail: String::new(),
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
                        self.pool.push(Candidate {
                            text: (*word).to_string(),
                            detail: String::new(),
                            kind,
                        });
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
                self.pool.push(Candidate {
                    text: word.to_string(),
                    detail: String::new(),
                    kind: Kind::Word,
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::CompletionItem;

    fn item(label: &str, kind: u32, detail: &str, insert: &str, sort: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind,
            detail: detail.to_string(),
            insert: insert.to_string(),
            sort: sort.to_string(),
            filter: label.to_string(),
            edit: None,
            documentation: String::new(),
        }
    }

    #[test]
    fn les_propositions_du_serveur_remplacent_les_locales() {
        let mut buffer = TextBuffer::from_str("fn calculer_total() {}\nfn main() { calc }\n", None);
        buffer.sync();
        buffer.cursor_line = 1;
        buffer.cursor_col = 16;
        let mut completion = Completion::new();
        completion.refresh(&buffer, Some(Language::Rust), None, MIN_PREFIX);
        assert_eq!(completion.source(), Source::Local);
        assert!(completion.active);

        let items = vec![
            item("calculer_total()", 3, "fn() -> u32", "calculer_total()", "aaa"),
            item("calc_interne", 6, "u32", "calc_interne", "bbb"),
            item("rien_a_voir", 6, "", "rien_a_voir", "ccc"),
        ];
        completion.set_language_items(&items, "calc");
        assert_eq!(completion.source(), Source::Language);
        assert_eq!(completion.items.len(), 2, "le candidat sans rapport est filtre");
        assert_eq!(completion.items[0].text, "calculer_total()");
        assert_eq!(completion.items[0].detail, "fn() -> u32");
        assert_eq!(completion.items[0].kind, Kind::Function);
        assert_eq!(completion.items[1].kind, Kind::Word);
        assert_eq!(completion.prefix_chars, 4);
        assert_eq!(completion.prefix(), "calc");
    }

    #[test]
    fn prefixe_vide_garde_l_ordre_du_serveur() {
        let mut completion = Completion::new();
        let items = vec![
            item("zzz", 5, "champ", "zzz", "b"),
            item("aaa", 7, "classe", "aaa", "a"),
        ];
        completion.set_language_items(&items, "");
        assert_eq!(completion.items.len(), 2);
        assert_eq!(completion.items[0].text, "aaa", "sortText departage");
        assert_eq!(completion.items[0].kind, Kind::Class);
        assert_eq!(completion.items[1].kind, Kind::Word);
        assert_eq!(completion.prefix_chars, 0);
    }

    #[test]
    fn une_reponse_vide_laisse_les_propositions_locales() {
        let mut buffer = TextBuffer::from_str("fn calculer() {}\nfn main() { calc }\n", None);
        buffer.sync();
        buffer.cursor_line = 1;
        buffer.cursor_col = 16;
        let mut completion = Completion::new();
        completion.refresh(&buffer, Some(Language::Rust), None, MIN_PREFIX);
        let avant = completion.items.len();
        assert!(avant > 0);
        completion.set_language_items(&[], "calc");
        assert_eq!(completion.items.len(), avant);
        assert_eq!(completion.source(), Source::Local);
    }

    #[test]
    fn la_table_des_genres_couvre_le_contrat() {
        assert_eq!(Kind::from_language(3), Kind::Function);
        assert_eq!(Kind::from_language(5), Kind::Word);
        assert_eq!(Kind::from_language(6), Kind::Word);
        assert_eq!(Kind::from_language(7), Kind::Class);
        assert_eq!(Kind::from_language(8), Kind::Interface);
        assert_eq!(Kind::from_language(9), Kind::Module);
        assert_eq!(Kind::from_language(10), Kind::Word);
        assert_eq!(Kind::from_language(14), Kind::Keyword);
        assert_eq!(Kind::from_language(22), Kind::Type);
        assert_eq!(Kind::from_language(23), Kind::Method);
        assert_eq!(Kind::from_language(25), Kind::Type);
        assert_eq!(Kind::from_language(0), Kind::Word);
    }

    #[test]
    fn le_rejet_revient_a_la_source_locale() {
        let mut completion = Completion::new();
        completion.set_language_items(&[item("abc", 3, "", "abc", "a")], "");
        assert_eq!(completion.source(), Source::Language);
        completion.dismiss();
        assert_eq!(completion.source(), Source::Local);
        assert!(!completion.active);
    }
}
