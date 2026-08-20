use crate::complete::{Completion, Kind, MIN_PREFIX};
use crate::syntax::{Highlighter, Language, STYLE_TEXT};
use crate::text::TextBuffer;

fn prepared(source: &str, language: Language) -> (TextBuffer, Highlighter) {
    let mut buffer = TextBuffer::from_str(source, None);
    buffer.sync();
    let mut highlighter = Highlighter::new(language).expect("grammaire indisponible");
    highlighter.update(buffer.source(), buffer.line_starts(), 0..buffer.source().len());
    (buffer, highlighter)
}

fn style_of_word(source: &str, highlighter: &Highlighter, word: &str) -> u8 {
    let at = source.find(word).expect("mot absent");
    highlighter.style_at(at)
}

#[test]
fn edition_insere_et_fusionne_les_lignes() {
    let mut buffer = TextBuffer::from_str("abc\ndef", None);
    buffer.cursor_col = 3;
    buffer.insert_char('!');
    assert_eq!(buffer.lines[0], "abc!");

    buffer.insert_newline();
    assert_eq!(buffer.lines.len(), 3);
    assert_eq!(buffer.cursor_line, 1);

    buffer.backspace();
    assert_eq!(buffer.lines.len(), 2);
    assert_eq!(buffer.lines[0], "abc!");
    assert_eq!(buffer.cursor_col, 4);
}

#[test]
fn tabulations_alignees_sur_quatre_colonnes() {
    let buffer = TextBuffer::from_str("a\tb\n\tc", None);
    assert_eq!(buffer.lines[0], "a   b");
    assert_eq!(buffer.lines[1], "    c");
}

#[test]
fn prefixe_de_mot_gere_les_accents() {
    let mut buffer = TextBuffer::from_str("let précédé = 1;", None);
    buffer.cursor_col = 11;
    assert_eq!(buffer.word_prefix(), "précédé");
    buffer.cursor_col = 4;
    assert_eq!(buffer.word_prefix(), "");
}

#[test]
fn coloration_rust_distingue_les_roles() {
    let source = "fn calculer(n: u32) -> u32 {\n    let texte = \"ok\";\n    n\n}\n";
    let (_, highlighter) = prepared(source, Language::Rust);
    let keyword = style_of_word(source, &highlighter, "fn ");
    let function = style_of_word(source, &highlighter, "calculer");
    let kind = style_of_word(source, &highlighter, "u32");
    let literal = style_of_word(source, &highlighter, "\"ok\"");
    assert_ne!(keyword, STYLE_TEXT);
    assert_ne!(function, STYLE_TEXT);
    assert_ne!(literal, STYLE_TEXT);
    assert_ne!(keyword, function);
    assert_ne!(keyword, kind);
    assert_ne!(function, literal);
}

#[test]
fn coloration_csharp_distingue_les_roles() {
    let source = "public class Sac {\n    public int Total() => 3;\n}\n";
    let (_, highlighter) = prepared(source, Language::CSharp);
    let keyword = style_of_word(source, &highlighter, "public");
    let kind = style_of_word(source, &highlighter, "Sac");
    let number = style_of_word(source, &highlighter, "3");
    assert_ne!(keyword, STYLE_TEXT);
    assert_ne!(kind, STYLE_TEXT);
    assert_ne!(number, STYLE_TEXT);
    assert_ne!(keyword, kind);
}

#[test]
fn reparse_incremental_suit_les_editions() {
    let mut buffer = TextBuffer::from_str("fn a() {}\n", None);
    buffer.sync();
    let mut highlighter = Highlighter::new(Language::Rust).unwrap();
    highlighter.update(buffer.source(), buffer.line_starts(), 0..buffer.source().len());

    buffer.cursor_line = 0;
    buffer.cursor_col = 0;
    buffer.insert_str("pub ");
    buffer.sync();
    highlighter.update(buffer.source(), buffer.line_starts(), 0..buffer.source().len());

    let source = buffer.source().to_string();
    assert_eq!(
        style_of_word(&source, &highlighter, "pub"),
        style_of_word(&source, &highlighter, "fn ")
    );
    assert_ne!(style_of_word(&source, &highlighter, "a()"), STYLE_TEXT);
}

#[test]
fn completion_propose_les_symboles_du_fichier_en_premier() {
    let source = "fn calculer_total() -> u32 { 0 }\nfn main() { calc }\n";
    let (mut buffer, highlighter) = prepared(source, Language::Rust);
    buffer.cursor_line = 1;
    buffer.cursor_col = 16;
    assert_eq!(buffer.word_prefix(), "calc");

    let mut completion = Completion::new();
    completion.refresh(&buffer, Some(Language::Rust), Some(&highlighter), MIN_PREFIX);

    assert!(completion.active);
    assert_eq!(completion.items[0].text, "calculer_total");
    assert_eq!(completion.items[0].kind, Kind::Function);
    assert_eq!(completion.prefix_chars, 4);
}

#[test]
fn completion_propose_les_mots_cles_du_langage() {
    let source = "class Sac {\n    void F() { ret }\n}\n";
    let (mut buffer, highlighter) = prepared(source, Language::CSharp);
    buffer.cursor_line = 1;
    buffer.cursor_col = 18;
    assert_eq!(buffer.word_prefix(), "ret");

    let mut completion = Completion::new();
    completion.refresh(&buffer, Some(Language::CSharp), Some(&highlighter), MIN_PREFIX);
    assert!(completion.items.iter().any(|item| item.text == "return"));
}

#[test]
fn completion_se_ferme_sous_le_prefixe_minimal() {
    let source = "fn calculer() {}\nfn main() { c }\n";
    let (mut buffer, highlighter) = prepared(source, Language::Rust);
    buffer.cursor_line = 1;
    buffer.cursor_col = 13;
    assert_eq!(buffer.word_prefix(), "c");

    let mut completion = Completion::new();
    completion.refresh(&buffer, Some(Language::Rust), Some(&highlighter), MIN_PREFIX);
    assert!(!completion.active);
    completion.refresh(&buffer, Some(Language::Rust), Some(&highlighter), 1);
    assert!(completion.active);
}

