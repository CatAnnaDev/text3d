use std::ops::Range;
use std::path::Path;

use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    CSharp,
}

impl Language {
    pub fn detect(path: Option<&Path>) -> Option<Language> {
        match path?.extension()?.to_str()? {
            "rs" => Some(Language::Rust),
            "cs" | "csx" => Some(Language::CSharp),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::CSharp => "c#",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        }
    }

    fn highlights(self) -> &'static str {
        match self {
            Language::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
            Language::CSharp => tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        }
    }

    fn tags(self) -> &'static str {
        match self {
            Language::Rust => tree_sitter_rust::TAGS_QUERY,
            Language::CSharp => tree_sitter_c_sharp::TAGS_QUERY,
        }
    }
}

pub const STYLE_TEXT: u8 = 0;
pub const STYLE_COUNT: usize = 19;

pub const STYLE_COLORS: [[u8; 4]; STYLE_COUNT] = [
    [200, 206, 222, 255],
    [201, 140, 240, 255],
    [111, 208, 200, 255],
    [111, 180, 247, 255],
    [143, 198, 255, 255],
    [240, 168, 96, 255],
    [255, 157, 106, 255],
    [154, 218, 122, 255],
    [240, 208, 112, 255],
    [242, 185, 106, 255],
    [ 92, 100, 128, 255],
    [109, 127, 160, 255],
    [217, 164, 95, 255],
    [154, 164, 189, 255],
    [126, 136, 163, 255],
    [127, 216, 255, 255],
    [214, 200, 232, 255],
    [127, 216, 255, 255],
    [240, 143, 176, 255],
];

fn style_of(capture: &str) -> u8 {
    let head = capture.split('.').next().unwrap_or(capture);
    match capture {
        "function.macro" => 5,
        "function.method" | "method" => 4,
        "comment.documentation" => 11,
        "string.escape" | "escape" => 8,
        "variable.parameter" => 16,
        "variable.builtin" => 6,
        _ => match head {
            "keyword" => 1,
            "type" | "constructor" => 2,
            "function" => 3,
            "constant" | "boolean" => 6,
            "string" | "character" => 7,
            "number" | "float" => 9,
            "comment" => 10,
            "attribute" | "annotation" => 12,
            "operator" => 13,
            "punctuation" | "delimiter" | "bracket" => 14,
            "property" | "field" | "module" | "namespace" => 15,
            "label" => 18,
            _ => STYLE_TEXT,
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Macro,
    Other,
}

impl SymbolKind {
    fn parse(tag: &str) -> SymbolKind {
        match tag {
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" | "struct" | "enum" => SymbolKind::Class,
            "interface" | "trait" => SymbolKind::Interface,
            "module" | "namespace" => SymbolKind::Module,
            "macro" => SymbolKind::Macro,
            _ => SymbolKind::Other,
        }
    }
}

pub struct Highlighter {
    parser: Parser,
    tree: Option<Tree>,
    highlight_query: Query,
    tags_query: Query,
    cursor: QueryCursor,
    capture_styles: Vec<u8>,
    name_capture: Option<u32>,
    styles: Vec<u8>,
    range: Range<usize>,
    parsed: String,
    spans: Vec<(usize, usize, u8, usize)>,
}

impl Highlighter {
    pub fn new(language: Language) -> Option<Highlighter> {
        let grammar = language.grammar();
        let mut parser = Parser::new();
        parser.set_language(&grammar).ok()?;
        let highlight_query = Query::new(&grammar, language.highlights()).ok()?;
        let tags_query = Query::new(&grammar, language.tags()).ok()?;
        let capture_styles = highlight_query
            .capture_names()
            .iter()
            .map(|name| style_of(name))
            .collect();
        let name_capture = tags_query
            .capture_names()
            .iter()
            .position(|name| *name == "name")
            .map(|index| index as u32);

        Some(Highlighter {
            parser,
            tree: None,
            highlight_query,
            tags_query,
            cursor: QueryCursor::new(),
            capture_styles,
            name_capture,
            styles: Vec::new(),
            range: 0..0,
            parsed: String::new(),
            spans: Vec::new(),
        })
    }

    pub fn update(&mut self, source: &str, line_starts: &[usize], window: Range<usize>) {
        if self.tree.is_none() || self.parsed != source {
            self.reparse(source, line_starts);
        }
        self.shade(source, window);
    }

    fn reparse(&mut self, source: &str, line_starts: &[usize]) {
        if let (Some(tree), Some(edit)) = (
            self.tree.as_mut(),
            diff(&self.parsed, source, line_starts),
        ) {
            tree.edit(&edit);
        }
        self.tree = self.parser.parse(source, self.tree.as_ref());
        self.parsed.clear();
        self.parsed.push_str(source);
    }

    fn shade(&mut self, source: &str, window: Range<usize>) {
        self.range = window.clone();
        self.styles.clear();
        self.styles.resize(window.len(), STYLE_TEXT);
        let Some(tree) = self.tree.as_ref() else {
            return;
        };

        let spans = &mut self.spans;
        spans.clear();
        self.cursor.set_byte_range(window.clone());
        let mut matches =
            self.cursor
                .matches(&self.highlight_query, tree.root_node(), source.as_bytes());
        while let Some(found) = matches.next() {
            for capture in found.captures {
                let style = self.capture_styles[capture.index as usize];
                if style == STYLE_TEXT {
                    continue;
                }
                let span = capture.node.byte_range();
                spans.push((span.start, span.end, style, found.pattern_index));
            }
        }

        spans.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.3.cmp(&a.3)));
        for &(start, end, style, _) in spans.iter() {
            let start = start.clamp(window.start, window.end) - window.start;
            let end = end.clamp(window.start, window.end) - window.start;
            self.styles[start..end].fill(style);
        }
    }

    pub fn style_at(&self, byte: usize) -> u8 {
        if byte < self.range.start || byte >= self.range.end {
            return STYLE_TEXT;
        }
        self.styles[byte - self.range.start]
    }

    pub fn collect_symbols(&self, source: &str, out: &mut Vec<(String, SymbolKind)>) {
        let (Some(tree), Some(name_capture)) = (self.tree.as_ref(), self.name_capture) else {
            return;
        };
        let names = self.tags_query.capture_names();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.tags_query, tree.root_node(), source.as_bytes());
        while let Some(found) = matches.next() {
            let mut symbol = None;
            let mut kind = None;
            for capture in found.captures {
                if capture.index == name_capture {
                    symbol = capture.node.utf8_text(source.as_bytes()).ok();
                } else if let Some(tag) = names[capture.index as usize].strip_prefix("definition.")
                {
                    kind = Some(SymbolKind::parse(tag));
                }
            }
            if let (Some(symbol), Some(kind)) = (symbol, kind) {
                out.push((symbol.to_string(), kind));
            }
        }
    }
}

fn diff(old: &str, new: &str, line_starts: &[usize]) -> Option<InputEdit> {
    if old == new {
        return None;
    }
    let (previous, current) = (old.as_bytes(), new.as_bytes());
    let overlap = previous.len().min(current.len());

    let mut start = 0;
    while start < overlap && previous[start] == current[start] {
        start += 1;
    }
    while start > 0 && !new.is_char_boundary(start) {
        start -= 1;
    }

    let mut back = 0;
    while back < overlap - start
        && previous[previous.len() - 1 - back] == current[current.len() - 1 - back]
    {
        back += 1;
    }
    while back > 0
        && (!old.is_char_boundary(previous.len() - back)
            || !new.is_char_boundary(current.len() - back))
    {
        back -= 1;
    }

    let old_end = previous.len() - back;
    let new_end = current.len() - back;
    let start_position = point_at(line_starts, start);

    Some(InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position,
        old_end_position: advance(start_position, &old[start..old_end]),
        new_end_position: advance(start_position, &new[start..new_end]),
    })
}

fn point_at(line_starts: &[usize], byte: usize) -> Point {
    let row = line_starts.partition_point(|start| *start <= byte).max(1) - 1;
    Point { row, column: byte - line_starts.get(row).copied().unwrap_or(0) }
}

fn advance(from: Point, text: &str) -> Point {
    let mut point = from;
    for byte in text.bytes() {
        if byte == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }
    point
}
