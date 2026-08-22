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


#[test]
fn enregistrer_ne_rajoute_pas_de_ligne_vide() {
    let dossier = std::env::temp_dir().join("text3d_aller_retour");
    let _ = std::fs::create_dir_all(&dossier);
    for original in ["fn a() {}\n", "sans saut final", "a\nb\n", "", "\n"] {
        let chemin = dossier.join("essai.rs");
        std::fs::write(&chemin, original).expect("ecriture");
        let mut tampon = TextBuffer::open(&chemin);
        tampon.save();
        let relu = std::fs::read_to_string(&chemin).expect("lecture");
        assert_eq!(relu, original, "aller retour instable pour {original:?}");
        tampon.save();
        let deux = std::fs::read_to_string(&chemin).expect("lecture");
        assert_eq!(deux, original, "le fichier grossit a chaque enregistrement");
    }
    let _ = std::fs::remove_dir_all(&dossier);
}

#[test]
fn defilement_de_la_sortie_accorde_entre_overlay_et_hud() {
    use crate::hud::CHAR_WIDTH;
    let mut overlay = crate::overlay::Overlay::new();
    overlay.set_output_rows(9);
    overlay.scroll_output(4, 40);
    assert_eq!(overlay.output_scroll(), 4);
    overlay.scroll_output(-2, 40);
    assert_eq!(overlay.output_scroll(), 2);
    overlay.follow_output(40);
    assert_eq!(overlay.output_scroll(), 2);
    overlay.scroll_output(100, 40);
    assert_eq!(overlay.output_scroll(), 31);
    assert!(overlay.output_follow());
    overlay.follow_output(60);
    assert_eq!(overlay.output_scroll(), 51);
    assert!(CHAR_WIDTH > 0.0);
}

#[test]
fn le_pointage_suit_le_relief_d_indentation() {
    use crate::camera::Camera;
    use crate::font::Font;
    use crate::layout::LineLayout;
    use crate::render::{INDENT_DEPTH, indent_level};
    use glam::Vec4;

    let Ok(font) = Font::load() else {
        return;
    };
    let source = "fn dehors() {\n    let a = 1;\n        let plus_loin = 2;\n                                let tres_profond = 3;\n}\n";
    let tampon = TextBuffer::from_str(source, None);
    let mut camera = Camera::new();
    camera.yaw = 0.85;
    camera.pitch = 0.22;
    camera.distance = 22.0;
    camera.target = glam::Vec3::new(14.0, -2.4, 0.0);

    let (largeur, hauteur) = (1280.0f32, 800.0f32);
    let aspect = largeur / hauteur;
    let vue = camera.view_proj(aspect);
    let mut layout = LineLayout::default();

    for (ligne, colonne) in [(1usize, 6usize), (2, 12), (3, 34), (3, 44), (0, 3)] {
        layout.build(&font, tampon.lines[ligne].as_str());
        let avance = layout
            .placements
            .get(colonne)
            .map(|place| place.advance)
            .unwrap_or(font.advance());
        let monde = glam::Vec3::new(
            layout.x_of_column(colonne) + avance * 0.25,
            -(ligne as f32) * font.line_height(),
            -(indent_level(tampon.lines[ligne].as_str()) as f32) * INDENT_DEPTH,
        );
        let clip = vue * Vec4::new(monde.x, monde.y, monde.z, 1.0);
        assert!(clip.w > 0.0, "point derriere la camera");
        let ecran_x = (clip.x / clip.w * 0.5 + 0.5) * largeur;
        let ecran_y = (0.5 - clip.y / clip.w * 0.5) * hauteur;

        let touche = crate::pick::hit_text(
            &camera,
            aspect,
            largeur,
            hauteur,
            ecran_x,
            ecran_y,
            &tampon,
            &font,
            &mut layout,
            INDENT_DEPTH,
        )
        .expect("le rayon doit toucher le texte");
        assert_eq!(touche.line, ligne, "ligne visee {ligne} colonne {colonne}");
        assert_eq!(touche.column, colonne, "colonne visee {ligne}:{colonne}");
    }
}
