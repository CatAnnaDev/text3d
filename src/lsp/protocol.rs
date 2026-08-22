use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::json::Json;
use crate::lsp::position::utf16_to_char;

const MAX_SYMBOL_DEPTH: usize = 64;
const HEX: &[u8; 16] = b"0123456789ABCDEF";

static NULL: Json = Json::Null;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub const fn new(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    pub const fn zero() -> Position {
        Position { line: 0, character: 0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub const fn new(start: Position, end: Position) -> Range {
        Range { start, end }
    }

    pub const fn empty(at: Position) -> Range {
        Range { start: at, end: at }
    }

    pub const fn zero() -> Range {
        Range { start: Position::zero(), end: Position::zero() }
    }
}

#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn from_code(code: i64) -> Severity {
        match code {
            1 => Severity::Error,
            2 => Severity::Warning,
            3 => Severity::Information,
            _ => Severity::Hint,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "erreur",
            Severity::Warning => "avertissement",
            Severity::Information => "information",
            Severity::Hint => "indice",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub code: String,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub kind: u32,
    pub detail: String,
    pub insert: String,
    pub sort: String,
    pub filter: String,
    pub edit: Option<(Range, String)>,
    pub documentation: String,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: u32,
    pub container: String,
    pub range: Range,
    pub selection: Range,
    pub path: PathBuf,
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub range: Range,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SignatureInfo {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<String>,
    pub active_parameter: usize,
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    Cow::Owned(path.to_string_lossy().into_owned().into_bytes())
}

#[cfg(unix)]
fn path_of_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_of_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.' || byte == b'_' || byte == b'~'
}

pub fn uri_from_path(path: &Path) -> String {
    let bytes = path_bytes(path);
    let mut out = String::with_capacity(bytes.len() + 16);
    out.push_str("file://");
    if bytes.first() != Some(&b'/') {
        out.push('/');
    }
    for &byte in bytes.iter() {
        if unreserved(byte) || byte == b'/' {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let trimmed = uri.trim();
    let scheme_end = trimmed.find(':')?;
    if !trimmed[..scheme_end].eq_ignore_ascii_case("file") {
        return None;
    }
    let rest = &trimmed[scheme_end + 1..];
    let body = if let Some(after) = rest.strip_prefix("//") {
        let slash = after.find('/')?;
        let host = &after[..slash];
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return None;
        }
        &after[slash..]
    } else if rest.starts_with('/') {
        rest
    } else {
        return None;
    };
    let cut = body.find(['?', '#']).unwrap_or(body.len());
    let body = &body[..cut];
    if body.is_empty() {
        return None;
    }
    let source = body.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let byte = source[index];
        if byte == b'%' && index + 2 < source.len() {
            match (hex_value(source[index + 1]), hex_value(source[index + 2])) {
                (Some(high), Some(low)) => {
                    decoded.push((high << 4) | low);
                    index += 3;
                    continue;
                }
                _ => {}
            }
        }
        decoded.push(byte);
        index += 1;
    }
    path_of_bytes(decoded)
}

fn field<'a>(value: &'a Json, key: &str) -> &'a Json {
    value.get(key).unwrap_or(&NULL)
}

fn text_of<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or("")
}

pub fn position_from_json(value: &Json) -> Option<Position> {
    let line = value.get("line")?.as_u32()?;
    let character = value.get("character")?.as_u32()?;
    Some(Position { line, character })
}

pub fn range_from_json(value: &Json) -> Option<Range> {
    let start = position_from_json(value.get("start")?)?;
    let end = position_from_json(value.get("end")?)?;
    Some(Range { start, end })
}

fn code_text(value: Option<&Json>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(number) = value.as_i64() {
        return number.to_string();
    }
    if let Some(inner) = value.get("value") {
        if let Some(text) = inner.as_str() {
            return text.to_string();
        }
        if let Some(number) = inner.as_i64() {
            return number.to_string();
        }
    }
    String::new()
}

pub fn parse_diagnostics(value: &Json, out: &mut Vec<Diagnostic>) {
    let Some(items) = value.as_array() else {
        return;
    };
    out.reserve(items.len());
    for item in items {
        let Some(range) = item.get("range").and_then(range_from_json) else {
            continue;
        };
        let severity = match item.get("severity").and_then(Json::as_i64) {
            Some(code) => Severity::from_code(code),
            None => Severity::Error,
        };
        let raw = text_of(item, "message");
        let message = if raw.is_empty() { "diagnostic sans message".to_string() } else { raw.to_string() };
        out.push(Diagnostic {
            range,
            severity,
            message,
            source: text_of(item, "source").to_string(),
            code: code_text(item.get("code")),
        });
    }
}

fn detail_of(item: &Json) -> String {
    let detail = text_of(item, "detail");
    if !detail.is_empty() {
        return detail.to_string();
    }
    let labels = field(item, "labelDetails");
    let suffix = text_of(labels, "detail");
    let description = text_of(labels, "description");
    if suffix.is_empty() {
        return description.to_string();
    }
    if description.is_empty() {
        return suffix.to_string();
    }
    let mut merged = String::with_capacity(suffix.len() + description.len() + 1);
    merged.push_str(suffix);
    merged.push(' ');
    merged.push_str(description);
    merged
}

fn completion_edit(value: &Json) -> Option<(Range, String)> {
    let range = value
        .get("replace")
        .and_then(range_from_json)
        .or_else(|| value.get("insert").and_then(range_from_json))
        .or_else(|| value.get("range").and_then(range_from_json))?;
    Some((range, text_of(value, "newText").to_string()))
}

pub fn parse_completion(value: &Json, out: &mut Vec<CompletionItem>) -> bool {
    let (items, incomplete) = match value.as_array() {
        Some(items) => (items, false),
        None => {
            let incomplete = value.get("isIncomplete").and_then(Json::as_bool).unwrap_or(false);
            match value.get("items").and_then(Json::as_array) {
                Some(items) => (items, incomplete),
                None => return incomplete,
            }
        }
    };
    out.reserve(items.len());
    for item in items {
        let label = text_of(item, "label");
        if label.is_empty() {
            continue;
        }
        let edit = item.get("textEdit").and_then(completion_edit);
        let insert = match item.get("insertText").and_then(Json::as_str) {
            Some(text) if !text.is_empty() => text.to_string(),
            _ => match edit.as_ref() {
                Some((_, text)) if !text.is_empty() => text.clone(),
                _ => label.to_string(),
            },
        };
        let sort = match item.get("sortText").and_then(Json::as_str) {
            Some(text) if !text.is_empty() => text.to_string(),
            _ => label.to_string(),
        };
        let filter = match item.get("filterText").and_then(Json::as_str) {
            Some(text) if !text.is_empty() => text.to_string(),
            _ => label.to_string(),
        };
        out.push(CompletionItem {
            label: label.to_string(),
            kind: item.get("kind").and_then(Json::as_u32).unwrap_or(0),
            detail: detail_of(item),
            insert,
            sort,
            filter,
            edit,
            documentation: markup_text(item.get("documentation")),
        });
    }
    incomplete
}

fn push_location(value: &Json, out: &mut Vec<Location>) {
    if let Some(uri) = value.get("uri").and_then(Json::as_str) {
        if let Some(path) = path_from_uri(uri) {
            let range = value.get("range").and_then(range_from_json).unwrap_or(Range::zero());
            out.push(Location { path, range });
        }
        return;
    }
    if let Some(uri) = value.get("targetUri").and_then(Json::as_str) {
        if let Some(path) = path_from_uri(uri) {
            let range = value
                .get("targetSelectionRange")
                .and_then(range_from_json)
                .or_else(|| value.get("targetRange").and_then(range_from_json))
                .unwrap_or(Range::zero());
            out.push(Location { path, range });
        }
    }
}

pub fn parse_locations(value: &Json, out: &mut Vec<Location>) {
    match value {
        Json::Array(items) => {
            out.reserve(items.len());
            for item in items {
                push_location(item, out);
            }
        }
        Json::Object(_) => push_location(value, out),
        _ => {}
    }
}

fn push_plain(text: &str, out: &mut String) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed == "---" || trimmed == "___" {
            continue;
        }
        if trimmed.is_empty() && (out.is_empty() || out.ends_with("\n\n")) {
            continue;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

fn push_markup(value: &Json, out: &mut String) {
    match value {
        Json::String(text) => push_plain(text, out),
        Json::Array(items) => {
            for item in items {
                push_markup(item, out);
            }
        }
        Json::Object(_) => {
            if let Some(text) = value.get("value").and_then(Json::as_str) {
                push_plain(text, out);
            }
        }
        _ => {}
    }
}

fn trim_tail(out: &mut String) {
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
}

pub fn markup_text(value: Option<&Json>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let mut out = String::new();
    push_markup(value, &mut out);
    trim_tail(&mut out);
    out
}

pub fn parse_hover(value: &Json, out: &mut String) {
    out.clear();
    push_markup(field(value, "contents"), out);
    trim_tail(out);
}

fn push_symbol(value: &Json, path: &Path, container: &str, depth: usize, out: &mut Vec<Symbol>) {
    if depth > MAX_SYMBOL_DEPTH {
        return;
    }
    let name = text_of(value, "name");
    if name.is_empty() {
        return;
    }
    let kind = value.get("kind").and_then(Json::as_u32).unwrap_or(0);
    if let Some(location) = value.get("location") {
        let Some(uri) = location.get("uri").and_then(Json::as_str) else {
            return;
        };
        let Some(target) = path_from_uri(uri) else {
            return;
        };
        let range = location.get("range").and_then(range_from_json).unwrap_or(Range::zero());
        out.push(Symbol {
            name: name.to_string(),
            kind,
            container: text_of(value, "containerName").to_string(),
            range,
            selection: range,
            path: target,
            depth,
        });
        return;
    }
    let Some(range) = value.get("range").and_then(range_from_json) else {
        return;
    };
    let selection = value.get("selectionRange").and_then(range_from_json).unwrap_or(range);
    out.push(Symbol {
        name: name.to_string(),
        kind,
        container: container.to_string(),
        range,
        selection,
        path: path.to_path_buf(),
        depth,
    });
    if let Some(children) = value.get("children").and_then(Json::as_array) {
        out.reserve(children.len());
        for child in children {
            push_symbol(child, path, name, depth + 1, out);
        }
    }
}

pub fn parse_document_symbols(value: &Json, path: &Path, out: &mut Vec<Symbol>) {
    let Some(items) = value.as_array() else {
        return;
    };
    out.reserve(items.len());
    for item in items {
        push_symbol(item, path, "", 0, out);
    }
}

pub fn parse_workspace_symbols(value: &Json, out: &mut Vec<Symbol>) {
    let Some(items) = value.as_array() else {
        return;
    };
    out.reserve(items.len());
    let empty = Path::new("");
    for item in items {
        push_symbol(item, empty, "", 0, out);
    }
}

fn slice_utf16(text: &str, start: u32, end: u32) -> String {
    let from = utf16_to_char(text, start);
    let to = utf16_to_char(text, end);
    if to <= from {
        return String::new();
    }
    text.chars().skip(from).take(to - from).collect()
}

pub fn parse_signature(value: &Json) -> Option<SignatureInfo> {
    let signatures = value.get("signatures").and_then(Json::as_array)?;
    if signatures.is_empty() {
        return None;
    }
    let active = value.get("activeSignature").and_then(Json::as_u32).unwrap_or(0) as usize;
    let chosen = signatures.get(active).or_else(|| signatures.first())?;
    let label = text_of(chosen, "label").to_string();
    let documentation = markup_text(chosen.get("documentation"));
    let mut parameters = Vec::new();
    if let Some(list) = chosen.get("parameters").and_then(Json::as_array) {
        parameters.reserve(list.len());
        for parameter in list {
            match parameter.get("label") {
                Some(Json::String(text)) => parameters.push(text.clone()),
                Some(Json::Array(bounds)) => {
                    let start = bounds.first().and_then(Json::as_u32).unwrap_or(0);
                    let end = bounds.get(1).and_then(Json::as_u32).unwrap_or(0);
                    parameters.push(slice_utf16(&label, start, end));
                }
                _ => parameters.push(String::new()),
            }
        }
    }
    let active_parameter = chosen
        .get("activeParameter")
        .and_then(Json::as_u32)
        .or_else(|| value.get("activeParameter").and_then(Json::as_u32))
        .unwrap_or(0) as usize;
    Some(SignatureInfo { label, documentation, parameters, active_parameter })
}

pub fn parse_text_edits(value: &Json, out: &mut Vec<TextEdit>) {
    let Some(items) = value.as_array() else {
        return;
    };
    out.reserve(items.len());
    for item in items {
        let Some(range) = item.get("range").and_then(range_from_json) else {
            continue;
        };
        out.push(TextEdit { range, text: text_of(item, "newText").to_string() });
    }
}

pub fn parse_workspace_edit(value: &Json, out: &mut Vec<(PathBuf, Vec<TextEdit>)>) {
    if let Some(changes) = value.get("changes").and_then(Json::as_object) {
        out.reserve(changes.len());
        for entry in changes {
            let Some(path) = path_from_uri(&entry.0) else {
                continue;
            };
            let mut edits = Vec::new();
            parse_text_edits(&entry.1, &mut edits);
            if !edits.is_empty() {
                out.push((path, edits));
            }
        }
    }
    let Some(documents) = value.get("documentChanges").and_then(Json::as_array) else {
        return;
    };
    for entry in documents {
        let Some(uri) = entry.path("textDocument.uri").and_then(Json::as_str) else {
            continue;
        };
        let Some(path) = path_from_uri(uri) else {
            continue;
        };
        let mut edits = Vec::new();
        parse_text_edits(field(entry, "edits"), &mut edits);
        if edits.is_empty() {
            continue;
        }
        match out.iter_mut().find(|slot| slot.0 == path) {
            Some(slot) => slot.1.append(&mut edits),
            None => out.push((path, edits)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Json {
        match Json::parse(text) {
            Ok(value) => value,
            Err(error) => panic!("json de test invalide: {error}"),
        }
    }

    fn same(left: Range, right: Range) -> bool {
        left.start == right.start && left.end == right.end
    }

    #[test]
    fn uri_aller_retour_simple() {
        let path = Path::new("/Users/anna/tmp/text3d/src/main.rs");
        let uri = uri_from_path(path);
        assert_eq!(uri, "file:///Users/anna/tmp/text3d/src/main.rs");
        assert_eq!(path_from_uri(&uri).as_deref(), Some(path));
    }

    #[test]
    fn uri_encode_espaces_accents_et_reserves() {
        let path = Path::new("/tmp/mon projet/ete #1/cafe?.rs");
        let uri = uri_from_path(path);
        assert!(!uri.contains(' '));
        assert!(uri.contains("%20"));
        assert!(uri.contains("%23"));
        assert!(uri.contains("%3F"));
        assert_eq!(path_from_uri(&uri).as_deref(), Some(path));
        let accents = Path::new("/tmp/donnees/reglage_ete.rs");
        assert_eq!(path_from_uri(&uri_from_path(accents)).as_deref(), Some(accents));
        let emoji = Path::new("/tmp/dossier/fichier.rs");
        assert_eq!(path_from_uri(&uri_from_path(emoji)).as_deref(), Some(emoji));
    }

    #[test]
    fn uri_aller_retour_non_ascii() {
        for raw in ["/tmp/cafe\u{e9}/na\u{ef}ve.rs", "/tmp/\u{4e2d}\u{6587}/\u{1f600}.cs", "/tmp/a b/c%d.rs"] {
            let path = Path::new(raw);
            let uri = uri_from_path(path);
            assert!(uri.is_ascii(), "uri non ascii: {uri}");
            assert_eq!(path_from_uri(&uri).as_deref(), Some(path), "aller retour casse pour {raw}");
        }
    }

    #[test]
    fn uri_formes_tolerees_et_refusees() {
        assert_eq!(path_from_uri("file:///tmp/a.rs").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(path_from_uri("file://localhost/tmp/a.rs").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(path_from_uri("FILE:///tmp/a.rs").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(path_from_uri("file:/tmp/a.rs").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(path_from_uri("file:///tmp/a.rs#L12").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(path_from_uri("http://exemple.org/a.rs"), None);
        assert_eq!(path_from_uri("file://serveur/tmp/a.rs"), None);
        assert_eq!(path_from_uri(""), None);
        assert_eq!(path_from_uri("file://"), None);
        assert_eq!(path_from_uri("file:///tmp/a%zz.rs").as_deref(), Some(Path::new("/tmp/a%zz.rs")));
    }

    #[test]
    fn definition_en_objet() {
        let value = json("{\"uri\":\"file:///tmp/a.rs\",\"range\":{\"start\":{\"line\":3,\"character\":4},\"end\":{\"line\":3,\"character\":9}}}");
        let mut out = Vec::new();
        parse_locations(&value, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, Path::new("/tmp/a.rs"));
        assert_eq!(out[0].range.start, Position::new(3, 4));
    }

    #[test]
    fn definition_en_tableau() {
        let value = json("[{\"uri\":\"file:///tmp/a.rs\",\"range\":{\"start\":{\"line\":1,\"character\":0},\"end\":{\"line\":1,\"character\":2}}},{\"uri\":\"file:///tmp/b.rs\",\"range\":{\"start\":{\"line\":5,\"character\":1},\"end\":{\"line\":5,\"character\":3}}}]");
        let mut out = Vec::new();
        parse_locations(&value, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].path, Path::new("/tmp/b.rs"));
    }

    #[test]
    fn definition_en_lien() {
        let value = json("[{\"originSelectionRange\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":1}},\"targetUri\":\"file:///tmp/c.rs\",\"targetRange\":{\"start\":{\"line\":7,\"character\":0},\"end\":{\"line\":9,\"character\":1}},\"targetSelectionRange\":{\"start\":{\"line\":7,\"character\":3},\"end\":{\"line\":7,\"character\":8}}}]");
        let mut out = Vec::new();
        parse_locations(&value, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, Path::new("/tmp/c.rs"));
        assert_eq!(out[0].range.start, Position::new(7, 3));
    }

    #[test]
    fn definition_nulle_ou_vide() {
        let mut out = Vec::new();
        parse_locations(&json("null"), &mut out);
        parse_locations(&json("[]"), &mut out);
        parse_locations(&json("{}"), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn completion_en_tableau() {
        let value = json("[{\"label\":\"len\",\"kind\":2,\"detail\":\"usize\"},{\"label\":\"push\",\"kind\":2}]");
        let mut out = Vec::new();
        let incomplete = parse_completion(&value, &mut out);
        assert!(!incomplete);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label, "len");
        assert_eq!(out[0].insert, "len");
        assert_eq!(out[0].sort, "len");
    }

    #[test]
    fn completion_en_objet_incomplet() {
        let value = json("{\"isIncomplete\":true,\"items\":[{\"label\":\"map\",\"sortText\":\"0001\",\"filterText\":\"map\",\"insertText\":\"map(\",\"textEdit\":{\"range\":{\"start\":{\"line\":2,\"character\":4},\"end\":{\"line\":2,\"character\":6}},\"newText\":\"map\"},\"documentation\":{\"kind\":\"markdown\",\"value\":\"```rust\\nfn map()\\n```\\ntransforme\"}}]}");
        let mut out = Vec::new();
        let incomplete = parse_completion(&value, &mut out);
        assert!(incomplete);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].insert, "map(");
        assert_eq!(out[0].sort, "0001");
        let edit = out[0].edit.as_ref().expect("edit attendu");
        assert_eq!(edit.0.start, Position::new(2, 4));
        assert_eq!(edit.1, "map");
        assert!(!out[0].documentation.contains("```"));
        assert!(out[0].documentation.contains("transforme"));
    }

    #[test]
    fn completion_insert_replace() {
        let value = json("{\"items\":[{\"label\":\"total\",\"textEdit\":{\"newText\":\"total\",\"insert\":{\"start\":{\"line\":1,\"character\":2},\"end\":{\"line\":1,\"character\":4}},\"replace\":{\"start\":{\"line\":1,\"character\":2},\"end\":{\"line\":1,\"character\":7}}}}]}");
        let mut out = Vec::new();
        parse_completion(&value, &mut out);
        let edit = out[0].edit.as_ref().expect("edit attendu");
        assert_eq!(edit.0.end, Position::new(1, 7));
    }

    #[test]
    fn hover_trois_formes() {
        let mut out = String::new();
        parse_hover(&json("{\"contents\":\"texte simple\"}"), &mut out);
        assert_eq!(out, "texte simple");
        parse_hover(&json("{\"contents\":{\"kind\":\"markdown\",\"value\":\"\\n```rust\\nfn f()\\n```\\n\\n---\\n\\ndoc\\n\\n\\n\"}}"), &mut out);
        assert_eq!(out, "fn f()\n\ndoc");
        assert!(!out.starts_with('\n'));
        assert!(!out.contains("\n\n\n"));
        parse_hover(&json("{\"contents\":[{\"language\":\"rust\",\"value\":\"struct A\"},\"note\"]}"), &mut out);
        assert!(out.contains("struct A"));
        assert!(out.contains("note"));
        parse_hover(&json("null"), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn diagnostics_completes() {
        let value = json("[{\"range\":{\"start\":{\"line\":10,\"character\":4},\"end\":{\"line\":10,\"character\":9}},\"severity\":2,\"code\":\"unused_variables\",\"source\":\"rustc\",\"message\":\"variable inutilisee\"},{\"range\":{\"start\":{\"line\":1,\"character\":0},\"end\":{\"line\":1,\"character\":1}},\"code\":1002,\"message\":\"\"}]");
        let mut out = Vec::new();
        parse_diagnostics(&value, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].severity, Severity::Warning);
        assert_eq!(out[0].source, "rustc");
        assert_eq!(out[0].code, "unused_variables");
        assert_eq!(out[1].severity, Severity::Error);
        assert_eq!(out[1].code, "1002");
        assert_eq!(out[1].message, "diagnostic sans message");
    }

    #[test]
    fn symboles_hierarchiques() {
        let value = json("[{\"name\":\"Renderer\",\"kind\":23,\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":40,\"character\":1}},\"selectionRange\":{\"start\":{\"line\":0,\"character\":7},\"end\":{\"line\":0,\"character\":15}},\"children\":[{\"name\":\"draw\",\"kind\":6,\"range\":{\"start\":{\"line\":5,\"character\":4},\"end\":{\"line\":9,\"character\":5}},\"selectionRange\":{\"start\":{\"line\":5,\"character\":7},\"end\":{\"line\":5,\"character\":11}}}]}]");
        let mut out = Vec::new();
        parse_document_symbols(&value, Path::new("/tmp/r.rs"), &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].depth, 0);
        assert_eq!(out[1].name, "draw");
        assert_eq!(out[1].depth, 1);
        assert_eq!(out[1].container, "Renderer");
        assert_eq!(out[1].path, Path::new("/tmp/r.rs"));
        assert!(same(out[1].selection, Range::new(Position::new(5, 7), Position::new(5, 11))));
    }

    #[test]
    fn symboles_plats() {
        let value = json("[{\"name\":\"Prof\",\"kind\":5,\"containerName\":\"DeepProf\",\"location\":{\"uri\":\"file:///tmp/p.cs\",\"range\":{\"start\":{\"line\":3,\"character\":0},\"end\":{\"line\":30,\"character\":1}}}}]");
        let mut out = Vec::new();
        parse_workspace_symbols(&value, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].container, "DeepProf");
        assert_eq!(out[0].path, Path::new("/tmp/p.cs"));
    }

    #[test]
    fn signature_avec_bornes_utf16() {
        let value = json("{\"signatures\":[{\"label\":\"fn tracer(x: f32, y: f32)\",\"parameters\":[{\"label\":[10,16]},{\"label\":[18,24]}]}],\"activeSignature\":0,\"activeParameter\":1}");
        let info = parse_signature(&value).expect("signature attendue");
        assert_eq!(info.parameters.len(), 2);
        assert_eq!(info.parameters[0], "x: f32");
        assert_eq!(info.parameters[1], "y: f32");
        assert_eq!(info.active_parameter, 1);
        assert!(parse_signature(&json("{\"signatures\":[]}")).is_none());
        assert!(parse_signature(&json("null")).is_none());
    }

    #[test]
    fn edition_espace_de_travail_deux_formes() {
        let par_changes = json("{\"changes\":{\"file:///tmp/a.rs\":[{\"range\":{\"start\":{\"line\":1,\"character\":0},\"end\":{\"line\":1,\"character\":3}},\"newText\":\"abc\"}]}}");
        let mut out = Vec::new();
        parse_workspace_edit(&par_changes, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.len(), 1);
        assert_eq!(out[0].1[0].text, "abc");
        let par_documents = json("{\"documentChanges\":[{\"textDocument\":{\"uri\":\"file:///tmp/b.rs\",\"version\":2},\"edits\":[{\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":1}},\"newText\":\"z\"}]},{\"kind\":\"rename\",\"oldUri\":\"file:///tmp/b.rs\",\"newUri\":\"file:///tmp/c.rs\"}]}");
        let mut autre = Vec::new();
        parse_workspace_edit(&par_documents, &mut autre);
        assert_eq!(autre.len(), 1);
        assert_eq!(autre[0].0, Path::new("/tmp/b.rs"));
    }

    #[test]
    fn formatage_liste_editions() {
        let value = json("[{\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":4}},\"newText\":\"    \"}]");
        let mut out = Vec::new();
        parse_text_edits(&value, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "    ");
        let mut vide = Vec::new();
        parse_text_edits(&json("null"), &mut vide);
        assert!(vide.is_empty());
    }

    #[test]
    fn severite_par_defaut_et_libelles() {
        assert_eq!(Severity::from_code(1), Severity::Error);
        assert_eq!(Severity::from_code(4), Severity::Hint);
        assert_eq!(Severity::from_code(99), Severity::Hint);
        assert_eq!(Severity::Warning.label(), "avertissement");
    }
}
