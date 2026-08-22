use std::fmt::Write as FmtWrite;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

const MAX_DEPTH: u32 = 256;

const fn build_escape_table() -> [bool; 256] {
    let mut table = [false; 256];
    let mut code = 0usize;
    while code < 0x20 {
        table[code] = true;
        code += 1;
    }
    table[b'"' as usize] = true;
    table[b'\\' as usize] = true;
    table
}

static NEEDS_ESCAPE: [bool; 256] = build_escape_table();
static HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";

impl Json {
    pub fn parse(input: &str) -> Result<Json, String> {
        let mut parser = Parser::new(input);
        let value = parser.value()?;
        parser.skip_spaces();
        if parser.position < parser.bytes.len() {
            return parser.fail("contenu inattendu apres la valeur");
        }
        Ok(value)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields
                .iter()
                .rev()
                .find(|(name, _)| name.as_str() == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn at(&self, index: usize) -> Option<&Json> {
        match self {
            Json::Array(items) => items.get(index),
            _ => None,
        }
    }

    pub fn path(&self, path: &str) -> Option<&Json> {
        let mut node = self;
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
            node = match node {
                Json::Array(items) => items.get(decimal_index(segment)?)?,
                Json::Object(_) => node.get(segment)?,
                _ => return None,
            };
        }
        Some(node)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(value) => {
                let value = *value;
                if value.fract() == 0.0
                    && (-9223372036854775808.0..9223372036854775808.0).contains(&value)
                {
                    Some(value as i64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|value| u32::try_from(value).ok())
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(fields) => Some(fields.as_slice()),
            _ => None,
        }
    }

    pub fn take_string(&mut self) -> Option<String> {
        match self {
            Json::String(_) => match std::mem::replace(self, Json::Null) {
                Json::String(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(value) => write_number(*value, out),
            Json::String(value) => write_string(value, out),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (index, (name, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(name, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(128);
        self.write(&mut out);
        out
    }

    pub fn str(value: &str) -> Json {
        Json::String(value.to_string())
    }

    pub fn num(value: f64) -> Json {
        Json::Number(value)
    }

    pub fn int(value: i64) -> Json {
        Json::Number(value as f64)
    }

    pub fn obj(fields: Vec<(&str, Json)>) -> Json {
        let mut out = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            out.push((name.to_string(), value));
        }
        Json::Object(out)
    }

    pub fn array(items: Vec<Json>) -> Json {
        Json::Array(items)
    }
}

fn decimal_index(segment: &str) -> Option<usize> {
    let bytes = segment.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut value: usize = 0;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    Some(value)
}

fn write_number(value: f64, out: &mut String) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    let magnitude = value.abs();
    if value.fract() == 0.0 && magnitude < 9007199254740992.0 {
        let _ = write!(out, "{}", value as i64);
        return;
    }
    if magnitude >= 1e17 || magnitude < 1e-5 {
        let _ = write!(out, "{value:e}");
        return;
    }
    let _ = write!(out, "{value}");
}

fn write_string(value: &str, out: &mut String) {
    let bytes = value.as_bytes();
    out.reserve(bytes.len() + 2);
    out.push('"');
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if !NEEDS_ESCAPE[byte as usize] {
            index += 1;
            continue;
        }
        if start < index {
            out.push_str(value.get(start..index).unwrap_or(""));
        }
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            _ => {
                out.push_str("\\u00");
                out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
                out.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
            }
        }
        index += 1;
        start = index;
    }
    if start < bytes.len() {
        out.push_str(value.get(start..).unwrap_or(""));
    }
    out.push('"');
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    position: usize,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Parser<'a> {
        let bytes = text.as_bytes();
        let position = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 };
        Parser { text, bytes, position, depth: 0 }
    }

    fn describe(&self, message: &str) -> String {
        let limit = self.position.min(self.bytes.len());
        let mut line = 1usize;
        let mut column = 1usize;
        let mut index = 0usize;
        while index < limit {
            let byte = self.bytes[index];
            if byte == b'\n' {
                line += 1;
                column = 1;
            } else if byte & 0xc0 != 0x80 {
                column += 1;
            }
            index += 1;
        }
        format!("ligne {line} colonne {column}: {message}")
    }

    fn fail<T>(&self, message: &str) -> Result<T, String> {
        Err(self.describe(message))
    }

    fn skip_spaces(&mut self) {
        while self.position < self.bytes.len() {
            match self.bytes[self.position] {
                b' ' | b'\t' | b'\n' | b'\r' => self.position += 1,
                _ => break,
            }
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_spaces();
        let byte = match self.bytes.get(self.position) {
            Some(byte) => *byte,
            None => return self.fail("valeur attendue, fin de donnees"),
        };
        match byte {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => {
                self.position += 1;
                Ok(Json::String(self.string_body()?))
            }
            b't' => self.keyword("true", Json::Bool(true)),
            b'f' => self.keyword("false", Json::Bool(false)),
            b'n' => self.keyword("null", Json::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => self.fail("valeur attendue"),
        }
    }

    fn keyword(&mut self, word: &str, value: Json) -> Result<Json, String> {
        let end = self.position + word.len();
        if self.bytes.get(self.position..end) == Some(word.as_bytes()) {
            self.position = end;
            Ok(value)
        } else {
            self.fail("mot cle invalide")
        }
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.fail("imbrication trop profonde, limite 256");
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Json, String> {
        self.position += 1;
        self.enter()?;
        self.skip_spaces();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Json::Object(Vec::new()));
        }
        let mut fields: Vec<(String, Json)> = Vec::with_capacity(4);
        loop {
            self.skip_spaces();
            if self.bytes.get(self.position) != Some(&b'"') {
                return self.fail("nom de champ attendu");
            }
            self.position += 1;
            let name = self.string_body()?;
            self.skip_spaces();
            if self.bytes.get(self.position) != Some(&b':') {
                return self.fail("deux points attendus apres le nom de champ");
            }
            self.position += 1;
            let value = self.value()?;
            match fields.iter().position(|(known, _)| *known == name) {
                Some(slot) => fields[slot].1 = value,
                None => fields.push((name, value)),
            }
            self.skip_spaces();
            match self.bytes.get(self.position) {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Json::Object(fields));
                }
                Some(_) => return self.fail("virgule ou accolade fermante attendue"),
                None => return self.fail("objet non termine"),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.position += 1;
        self.enter()?;
        self.skip_spaces();
        if self.bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Json::Array(Vec::new()));
        }
        let mut items: Vec<Json> = Vec::with_capacity(4);
        loop {
            items.push(self.value()?);
            self.skip_spaces();
            match self.bytes.get(self.position) {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Json::Array(items));
                }
                Some(_) => return self.fail("virgule ou crochet fermant attendu"),
                None => return self.fail("tableau non termine"),
            }
        }
    }

    fn string_body(&mut self) -> Result<String, String> {
        let start = self.position;
        let length = self.bytes.len();
        let mut index = start;
        while index < length {
            let byte = self.bytes[index];
            if !NEEDS_ESCAPE[byte as usize] {
                index += 1;
                continue;
            }
            if byte == b'"' {
                let slice = match self.text.get(start..index) {
                    Some(slice) => slice,
                    None => return self.fail("chaine mal formee"),
                };
                self.position = index + 1;
                return Ok(slice.to_string());
            }
            if byte == b'\\' {
                return self.escaped_string_body(start, index);
            }
            self.position = index;
            return self.fail("caractere de controle non echappe dans une chaine");
        }
        self.position = length;
        self.fail("chaine non terminee")
    }

    fn escaped_string_body(&mut self, start: usize, first: usize) -> Result<String, String> {
        let length = self.bytes.len();
        let head = match self.text.get(start..first) {
            Some(head) => head,
            None => return self.fail("chaine mal formee"),
        };
        let mut out = String::with_capacity(head.len() + 32);
        out.push_str(head);
        let mut index = first;
        while index < length {
            let byte = self.bytes[index];
            if byte == b'"' {
                self.position = index + 1;
                return Ok(out);
            }
            if byte == b'\\' {
                index += 1;
                let escape = match self.bytes.get(index) {
                    Some(escape) => *escape,
                    None => {
                        self.position = index;
                        return self.fail("echappement incomplet");
                    }
                };
                index += 1;
                match escape {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let unit = self.hex_quad(index)?;
                        index += 4;
                        if (0xd800..0xdc00).contains(&unit) {
                            if self.bytes.get(index) != Some(&b'\\')
                                || self.bytes.get(index + 1) != Some(&b'u')
                            {
                                self.position = index;
                                return self.fail("paire de substitution utf-16 incomplete");
                            }
                            let low = self.hex_quad(index + 2)?;
                            if !(0xdc00..0xe000).contains(&low) {
                                self.position = index;
                                return self.fail("paire de substitution utf-16 incomplete");
                            }
                            index += 6;
                            let code = 0x10000u32
                                + (((unit - 0xd800) as u32) << 10)
                                + (low - 0xdc00) as u32;
                            match char::from_u32(code) {
                                Some(value) => out.push(value),
                                None => {
                                    self.position = index;
                                    return self.fail("point de code invalide");
                                }
                            }
                        } else if (0xdc00..0xe000).contains(&unit) {
                            self.position = index;
                            return self.fail("substitution utf-16 basse isolee");
                        } else {
                            match char::from_u32(unit as u32) {
                                Some(value) => out.push(value),
                                None => {
                                    self.position = index;
                                    return self.fail("point de code invalide");
                                }
                            }
                        }
                    }
                    _ => {
                        self.position = index;
                        return self.fail("echappement inconnu");
                    }
                }
                continue;
            }
            if byte < 0x20 {
                self.position = index;
                return self.fail("caractere de controle non echappe dans une chaine");
            }
            let chunk = index;
            while index < length && !NEEDS_ESCAPE[self.bytes[index] as usize] {
                index += 1;
            }
            match self.text.get(chunk..index) {
                Some(slice) => out.push_str(slice),
                None => return self.fail("chaine mal formee"),
            }
        }
        self.position = length;
        self.fail("chaine non terminee")
    }

    fn hex_quad(&mut self, at: usize) -> Result<u16, String> {
        self.position = at.min(self.bytes.len());
        let slice = match self.bytes.get(at..at + 4) {
            Some(slice) => slice,
            None => return self.fail("echappement unicode incomplet"),
        };
        let mut value: u16 = 0;
        for byte in slice {
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return self.fail("chiffre hexadecimal invalide"),
            };
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.position;
        let mut index = start;
        let negative = self.bytes[index] == b'-';
        if negative {
            index += 1;
        }
        let digits = index;
        match self.bytes.get(index) {
            Some(b'0') => index += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
                    index += 1;
                }
            }
            _ => {
                self.position = index;
                return self.fail("chiffre attendu");
            }
        }
        if self.bytes[digits] == b'0' && matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
            self.position = index;
            return self.fail("zero initial superflu");
        }
        let integer_end = index;
        let mut plain = true;
        if self.bytes.get(index) == Some(&b'.') {
            plain = false;
            index += 1;
            if !matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
                self.position = index;
                return self.fail("chiffre attendu apres le point decimal");
            }
            while matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
                index += 1;
            }
        }
        if matches!(self.bytes.get(index), Some(b'e' | b'E')) {
            plain = false;
            index += 1;
            if matches!(self.bytes.get(index), Some(b'+' | b'-')) {
                index += 1;
            }
            if !matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
                self.position = index;
                return self.fail("chiffre attendu dans l exposant");
            }
            while matches!(self.bytes.get(index), Some(b'0'..=b'9')) {
                index += 1;
            }
        }
        self.position = index;
        if plain && integer_end - digits <= 18 {
            let mut value: i64 = 0;
            let mut cursor = digits;
            while cursor < integer_end {
                value = value * 10 + (self.bytes[cursor] - b'0') as i64;
                cursor += 1;
            }
            let value = value as f64;
            return Ok(Json::Number(if negative { -value } else { value }));
        }
        let text = match self.text.get(start..index) {
            Some(text) => text,
            None => return self.fail("nombre mal forme"),
        };
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Json::Number(value)),
            _ => self.fail("nombre hors limites"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn round_trip(text: &str) -> String {
        match Json::parse(text) {
            Ok(value) => value.to_text(),
            Err(message) => panic!("analyse refusee pour {text}: {message}"),
        }
    }

    #[test]
    fn scalars() {
        assert_eq!(Json::parse("null"), Ok(Json::Null));
        assert_eq!(Json::parse(" true "), Ok(Json::Bool(true)));
        assert_eq!(Json::parse("false"), Ok(Json::Bool(false)));
        assert_eq!(Json::parse("\"\""), Ok(Json::String(String::new())));
        assert_eq!(Json::parse("0"), Ok(Json::Number(0.0)));
        assert_eq!(round_trip("null"), "null");
        assert_eq!(round_trip("true"), "true");
    }

    #[test]
    fn nested_round_trip() {
        let source = "{\"result\":{\"items\":[{\"label\":\"push\",\"kind\":2,\"detail\":null},\
{\"label\":\"pop\",\"kind\":2,\"detail\":[1,2.5,-3,true,false,null]}],\"isIncomplete\":false},\
\"jsonrpc\":\"2.0\",\"id\":7}";
        let first = round_trip(source);
        let second = round_trip(&first);
        assert_eq!(first, second);
        let value = Json::parse(&first).expect("relecture");
        assert_eq!(value.path("result.items.1.label").and_then(Json::as_str), Some("pop"));
        assert_eq!(value.path("result.items.0.kind").and_then(Json::as_u32), Some(2));
        assert_eq!(value.path("result.items.1.detail.2").and_then(Json::as_i64), Some(-3));
        assert_eq!(value.path("id").and_then(Json::as_i64), Some(7));
        assert_eq!(value.path("result.isIncomplete").and_then(Json::as_bool), Some(false));
        assert_eq!(value.path("result.items.9"), None);
        assert_eq!(value.path("result.items.x"), None);
        assert_eq!(value.path("absent.chose"), None);
        assert_eq!(value.path(""), Some(&value));
    }

    #[test]
    fn accessors() {
        let value = Json::parse("{\"a\":[10,\"x\"],\"b\":\"y\"}").expect("analyse");
        assert_eq!(value.get("b").and_then(Json::as_str), Some("y"));
        assert_eq!(value.get("c"), None);
        assert_eq!(value.get("a").and_then(|node| node.at(1)).and_then(Json::as_str), Some("x"));
        assert_eq!(value.at(0), None);
        assert_eq!(value.as_array(), None);
        assert_eq!(value.as_object().map(<[(String, Json)]>::len), Some(2));
        assert_eq!(value.get("a").and_then(Json::as_array).map(<[Json]>::len), Some(2));
        assert_eq!(value.as_str(), None);
        assert_eq!(value.as_f64(), None);
        assert_eq!(value.as_bool(), None);
        let mut owned = Json::parse("\"contenu\"").expect("analyse");
        assert_eq!(owned.take_string().as_deref(), Some("contenu"));
        assert_eq!(owned, Json::Null);
        assert_eq!(owned.take_string(), None);
    }

    #[test]
    fn builders() {
        let value = Json::obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::int(12)),
            ("params", Json::array(vec![Json::num(0.5), Json::Bool(true), Json::Null])),
        ]);
        assert_eq!(
            value.to_text(),
            "{\"jsonrpc\":\"2.0\",\"id\":12,\"params\":[0.5,true,null]}"
        );
        assert_eq!(Json::obj(Vec::new()).to_text(), "{}");
        assert_eq!(Json::array(Vec::new()).to_text(), "[]");
    }

    #[test]
    fn escapes_are_read_and_written() {
        let value = Json::parse("\"a\\\"b\\\\c\\/d\\be\\ff\\ng\\rh\\ti\"").expect("analyse");
        assert_eq!(value.as_str(), Some("a\"b\\c/d\u{8}e\u{c}f\ng\rh\ti"));
        assert_eq!(
            value.to_text(),
            "\"a\\\"b\\\\c/d\\be\\ff\\ng\\rh\\ti\""
        );
    }

    #[test]
    fn control_characters_are_escaped_on_write() {
        let value = Json::String("\u{1}\u{1f}\u{7f}fin".to_string());
        assert_eq!(value.to_text(), "\"\\u0001\\u001f\u{7f}fin\"");
        let reread = Json::parse(&value.to_text()).expect("analyse");
        assert_eq!(reread, value);
    }

    #[test]
    fn surrogate_pairs() {
        let value = Json::parse("\"\\ud83d\\ude00\"").expect("analyse");
        assert_eq!(value.as_str(), Some("\u{1f600}"));
        assert_eq!(value.as_str().map(|text| text.chars().count()), Some(1));
        let value = Json::parse("\"a\\uD83D\\uDE00b\\u00e9\\u0041\"").expect("analyse");
        assert_eq!(value.as_str(), Some("a\u{1f600}b\u{e9}A"));
        assert_eq!(value.to_text(), "\"a\u{1f600}b\u{e9}A\"");
        let value = Json::parse("\"\\uD834\\uDD1E\"").expect("analyse");
        assert_eq!(value.as_str(), Some("\u{1d11e}"));
        let value = Json::parse("\"\\uDBFF\\uDFFF\"").expect("analyse");
        assert_eq!(value.as_str(), Some("\u{10ffff}"));
        assert!(Json::parse("\"\\ud83d\"").is_err());
        assert!(Json::parse("\"\\ud83dx\"").is_err());
        assert!(Json::parse("\"\\ud83d\\u0041\"").is_err());
        assert!(Json::parse("\"\\ude00\"").is_err());
    }

    #[test]
    fn utf8_passes_through() {
        let source = "{\"cle\":\"ete cafe \u{4e2d}\u{6587} \u{1f600}\"}";
        assert_eq!(round_trip(source), source);
    }

    #[test]
    fn numbers() {
        assert_eq!(Json::parse("0").ok().and_then(|v| v.as_f64()), Some(0.0));
        assert_eq!(Json::parse("-0").ok().and_then(|v| v.as_f64()), Some(-0.0));
        assert_eq!(Json::parse("1e3").ok().and_then(|v| v.as_f64()), Some(1000.0));
        assert_eq!(Json::parse("1E3").ok().and_then(|v| v.as_f64()), Some(1000.0));
        assert_eq!(Json::parse("1e+3").ok().and_then(|v| v.as_f64()), Some(1000.0));
        assert_eq!(Json::parse("2.5e-3").ok().and_then(|v| v.as_f64()), Some(0.0025));
        assert_eq!(Json::parse("-12.75").ok().and_then(|v| v.as_f64()), Some(-12.75));
        assert_eq!(Json::parse("9007199254740991").ok().and_then(|v| v.as_i64()), Some(9007199254740991));
        assert_eq!(Json::parse("123456789012345678").ok().and_then(|v| v.as_i64()), Some(123456789012345680));
        assert_eq!(Json::parse("9223372036854775808").ok().and_then(|v| v.as_i64()), None);
        assert_eq!(Json::parse("-9223372036854777000").ok().and_then(|v| v.as_i64()), None);
        assert_eq!(Json::parse("-9007199254740992").ok().and_then(|v| v.as_i64()), Some(-9007199254740992));
        assert_eq!(Json::parse("1.5").ok().and_then(|v| v.as_i64()), None);
        assert_eq!(Json::parse("1e300").ok().and_then(|v| v.as_i64()), None);
        assert_eq!(Json::parse("4294967295").ok().and_then(|v| v.as_u32()), Some(u32::MAX));
        assert_eq!(Json::parse("4294967296").ok().and_then(|v| v.as_u32()), None);
        assert_eq!(Json::parse("-1").ok().and_then(|v| v.as_u32()), None);
        assert_eq!(round_trip("1e3"), "1000");
        assert_eq!(round_trip("2.5"), "2.5");
        assert_eq!(round_trip("-0.125"), "-0.125");
        assert_eq!(Json::Number(f64::NAN).to_text(), "null");
        assert_eq!(Json::Number(f64::INFINITY).to_text(), "null");
        assert_eq!(Json::Number(f64::NEG_INFINITY).to_text(), "null");
        assert_eq!(Json::Number(f64::MAX).to_text(), "1.7976931348623157e308");
        assert_eq!(Json::Number(f64::MIN_POSITIVE).to_text(), "2.2250738585072014e-308");
        assert_eq!(Json::Number(5e-324).to_text(), "5e-324");
        assert_eq!(Json::Number(1e300).to_text(), "1e300");
        assert_eq!(Json::Number(-1e300).to_text(), "-1e300");
        assert_eq!(Json::Number(0.0001).to_text(), "0.0001");
        for probe in [f64::MAX, f64::MIN, f64::MIN_POSITIVE, 5e-324, 1e300, -1e300, 0.1, 1e17, -2.5e-9, 1234.5678] {
            let text = Json::Number(probe).to_text();
            assert_eq!(Json::parse(&text).ok().and_then(|v| v.as_f64()), Some(probe), "{text}");
        }
    }

    #[test]
    fn duplicate_key_keeps_last() {
        let value = Json::parse("{\"a\":1,\"b\":2,\"a\":3}").expect("analyse");
        assert_eq!(value.get("a").and_then(Json::as_i64), Some(3));
        assert_eq!(value.as_object().map(<[(String, Json)]>::len), Some(2));
    }

    #[test]
    fn malformed_inputs_are_refused() {
        let cases = [
            "",
            "   ",
            "{",
            "[1,2",
            "{\"a\":}",
            "{\"a\" 1}",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "1e",
            "-",
            "+1",
            ".5",
            "\"abc",
            "\"\\q\"",
            "\"\\u12\"",
            "\"\\uzzzz\"",
            "\"a\nb\"",
            "tru",
            "nul",
            "[1] extra",
            "{\"a\":1}{",
            "1e400",
            "{'a':1}",
            "[,1]",
            "{:1}",
            "\"\\",
        ];
        for case in cases {
            assert!(Json::parse(case).is_err(), "aurait du echouer: {case:?}");
        }
        assert_eq!(cases.len(), 28);
    }

    #[test]
    fn nesting_is_bounded() {
        let mut deep = String::with_capacity(200_002);
        for _ in 0..100_000 {
            deep.push('[');
        }
        for _ in 0..100_000 {
            deep.push(']');
        }
        let error = Json::parse(&deep).expect_err("profondeur refusee");
        assert!(error.contains("imbrication trop profonde"), "{error}");

        let mut deep_object = String::new();
        for _ in 0..100_000 {
            deep_object.push_str("{\"a\":");
        }
        assert!(Json::parse(&deep_object).is_err());

        let mut acceptable = String::new();
        for _ in 0..256 {
            acceptable.push('[');
        }
        acceptable.push('1');
        for _ in 0..256 {
            acceptable.push(']');
        }
        assert!(Json::parse(&acceptable).is_ok());

        let mut refused = String::new();
        for _ in 0..257 {
            refused.push('[');
        }
        refused.push('1');
        for _ in 0..257 {
            refused.push(']');
        }
        assert!(Json::parse(&refused).is_err());
    }

    #[test]
    fn no_panic_on_arbitrary_bytes() {
        let mut sample = String::new();
        let seeds = ["{", "}", "[", "]", ",", ":", "\"", "\\", "u", "0", "e", "-", ".", "\u{e9}", "\u{1f600}", "\n", "t", "n"];
        let mut state: usize = 12345;
        for _ in 0..40_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            sample.push_str(seeds[(state >> 33) % seeds.len()]);
            let _ = Json::parse(&sample);
            if sample.len() > 2000 {
                sample.clear();
            }
        }
    }

    #[test]
    fn large_array() {
        let mut source = String::with_capacity(700_000);
        source.push('[');
        for index in 0..100_000u32 {
            if index > 0 {
                source.push(',');
            }
            let _ = write!(source, "{index}");
        }
        source.push(']');
        let value = Json::parse(&source).expect("analyse du grand tableau");
        let items = value.as_array().expect("tableau");
        assert_eq!(items.len(), 100_000);
        assert_eq!(items[99_999].as_u32(), Some(99_999));
        assert_eq!(value.to_text(), source);
    }

    fn benchmark_document() -> String {
        let mut source = String::with_capacity(9_000_000);
        source.push_str("{\"jsonrpc\":\"2.0\",\"id\":42,\"result\":{\"isIncomplete\":false,\"items\":[");
        for index in 0..20_000u32 {
            if index > 0 {
                source.push(',');
            }
            let _ = write!(
                source,
                "{{\"label\":\"fonction_{index}\",\"kind\":3,\"detail\":\"fn(&mut self, valeur: u32) -> Option<&str>\",\
\"sortText\":\"ffff{index}\",\"filterText\":\"fonction_{index}\",\"preselect\":false,\"deprecated\":false,\
\"documentation\":{{\"kind\":\"markdown\",\"value\":\"ligne un\\nligne deux \\\"citee\\\" et un accent \u{e9}\"}},\
\"textEdit\":{{\"range\":{{\"start\":{{\"line\":{index},\"character\":0}},\"end\":{{\"line\":{index},\"character\":12}}}},\
\"newText\":\"fonction_{index}\"}},\"additionalTextEdits\":[],\"score\":0.5}}"
            );
        }
        source.push_str("]}}");
        source
    }

    #[test]
    fn throughput() {
        let source = benchmark_document();
        let bytes = source.len() as f64;
        let rounds = 5;
        let value = Json::parse(&source).expect("analyse de reference");
        assert_eq!(value.path("result.items.19999.label").and_then(Json::as_str), Some("fonction_19999"));
        assert_eq!(value.path("result.items.0.textEdit.range.end.character").and_then(Json::as_u32), Some(12));
        drop(value);
        let start = Instant::now();
        let mut checksum = 0usize;
        for _ in 0..rounds {
            let value = Json::parse(&source).expect("analyse");
            checksum += value.path("result.items").and_then(Json::as_array).map_or(0, <[Json]>::len);
        }
        let elapsed = start.elapsed().as_secs_f64();
        assert_eq!(checksum, 100_000);
        let rate = bytes * rounds as f64 / elapsed / 1_000_000.0;
        eprintln!("debit analyse json: {rate:.1} Mo/s sur {:.2} Mo", bytes / 1_000_000.0);

        let parsed = Json::parse(&source).expect("analyse");
        let mut out = String::with_capacity(source.len() + 1024);
        let start = Instant::now();
        for _ in 0..rounds {
            out.clear();
            parsed.write(&mut out);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let write_rate = out.len() as f64 * rounds as f64 / elapsed / 1_000_000.0;
        eprintln!("debit ecriture json: {write_rate:.1} Mo/s");
        assert_eq!(Json::parse(&out).expect("relecture"), parsed);

        let floor = if cfg!(debug_assertions) { 5.0 } else { 100.0 };
        assert!(rate >= floor, "debit trop faible: {rate:.1} Mo/s");
        assert!(write_rate >= floor, "ecriture trop lente: {write_rate:.1} Mo/s");
    }

    #[test]
    fn cargo_metadata_round_trip() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let output = std::process::Command::new(cargo)
            .args(["metadata", "--format-version", "1", "--no-deps", "--manifest-path"])
            .arg(&manifest)
            .output();
        let output = match output {
            Ok(output) if output.status.success() => output,
            _ => return,
        };
        let dump = std::env::temp_dir().join("text3d_cargo_metadata.json");
        if std::fs::write(&dump, &output.stdout).is_err() {
            return;
        }
        let text = match std::fs::read_to_string(&dump) {
            Ok(text) => text,
            Err(_) => return,
        };
        let value = Json::parse(&text).expect("analyse de cargo metadata");
        assert_eq!(value.get("version").and_then(Json::as_i64), Some(1));
        let packages = value.get("packages").and_then(Json::as_array).expect("packages");
        assert!(!packages.is_empty());
        assert!(packages[0].get("name").and_then(Json::as_str).is_some());
        assert!(packages[0].get("manifest_path").and_then(Json::as_str).is_some());
        assert!(value.get("workspace_root").and_then(Json::as_str).is_some());
        assert!(value.path("packages.0.targets.0.kind.0").and_then(Json::as_str).is_some());
        let written = value.to_text();
        let again = Json::parse(&written).expect("relecture");
        assert_eq!(again, value);
        let _ = std::fs::remove_file(&dump);
    }
}
