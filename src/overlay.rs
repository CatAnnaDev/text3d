use std::path::PathBuf;

use crate::text::Cursor;

pub const VISIBLE_ROWS: usize = 16;
pub const OUTPUT_ROWS: usize = 12;
pub const TREE_ROWS: usize = 24;
pub const MAX_QUERY: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    None,
    QuickOpen,
    DocumentSymbols,
    WorkspaceSymbols,
    Search,
    Problems,
    References,
    Rename,
    Commands,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Panel::None => "",
            Panel::QuickOpen => "ouverture rapide",
            Panel::DocumentSymbols => "symboles du document",
            Panel::WorkspaceSymbols => "symboles du projet",
            Panel::Search => "recherche dans le projet",
            Panel::Problems => "problemes",
            Panel::References => "references",
            Panel::Rename => "renommer",
            Panel::Commands => "commandes",
        }
    }

    pub fn takes_query(self) -> bool {
        matches!(
            self,
            Panel::QuickOpen
                | Panel::DocumentSymbols
                | Panel::WorkspaceSymbols
                | Panel::Search
                | Panel::Rename
                | Panel::Commands
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    File,
    Symbol,
    Match,
    Reference,
    Command,
    Error,
    Warning,
    Information,
    Hint,
    Plain,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub tag: &'static str,
    pub kind: RowKind,
    pub target: Option<(PathBuf, Cursor)>,
}

impl Row {
    pub fn new(
        label: String,
        detail: String,
        tag: &'static str,
        kind: RowKind,
        target: Option<(PathBuf, Cursor)>,
    ) -> Row {
        Row {
            label,
            detail,
            tag,
            kind,
            target,
        }
    }

    pub fn plain(label: String) -> Row {
        Row {
            label,
            detail: String::new(),
            tag: "",
            kind: RowKind::Plain,
            target: None,
        }
    }
}

pub struct Overlay {
    panel: Panel,
    query: String,
    search_query: String,
    rows: Vec<Row>,
    selection: usize,
    scroll: usize,
    status: String,
    sidebar: bool,
    sidebar_selection: usize,
    tree_scroll: usize,
    tree_rows: usize,
    output: bool,
    output_scroll: usize,
    output_rows: usize,
    output_follow: bool,
}

impl Default for Overlay {
    fn default() -> Overlay {
        Overlay::new()
    }
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay {
            panel: Panel::None,
            query: String::new(),
            search_query: String::new(),
            rows: Vec::new(),
            selection: 0,
            scroll: 0,
            status: String::new(),
            sidebar: true,
            sidebar_selection: 0,
            tree_scroll: 0,
            tree_rows: TREE_ROWS,
            output: false,
            output_scroll: 0,
            output_rows: OUTPUT_ROWS,
            output_follow: true,
        }
    }

    pub fn panel(&self) -> Panel {
        self.panel
    }

    pub fn open(&mut self, panel: Panel) {
        if panel == Panel::None {
            self.close();
            return;
        }
        if self.panel == panel {
            return;
        }
        self.remember_query();
        self.panel = panel;
        self.rows.clear();
        self.selection = 0;
        self.scroll = 0;
        self.status.clear();
        self.query.clear();
        if panel == Panel::Search {
            self.query.push_str(&self.search_query);
        }
    }

    pub fn open_with(&mut self, panel: Panel, query: String) {
        if panel == Panel::None {
            self.close();
            return;
        }
        if self.panel != panel {
            self.open(panel);
        }
        self.set_query(query);
    }

    pub fn close(&mut self) {
        if self.panel == Panel::None {
            return;
        }
        self.remember_query();
        self.panel = Panel::None;
        self.rows.clear();
        self.query.clear();
        self.status.clear();
        self.selection = 0;
        self.scroll = 0;
    }

    pub fn is_capturing_input(&self) -> bool {
        self.panel != Panel::None
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: String) {
        if self.query == query {
            return;
        }
        self.query = query;
        self.query.truncate(floor_boundary(&self.query, MAX_QUERY));
        self.selection = 0;
        self.scroll = 0;
    }

    pub fn insert(&mut self, ch: char) {
        if self.panel == Panel::None || ch.is_control() {
            return;
        }
        if self.query.len() + ch.len_utf8() > MAX_QUERY {
            return;
        }
        self.query.push(ch);
        self.selection = 0;
        self.scroll = 0;
    }

    pub fn backspace(&mut self) {
        if self.panel == Panel::None || self.query.is_empty() {
            return;
        }
        self.query.pop();
        self.selection = 0;
        self.scroll = 0;
    }

    pub fn clear_query(&mut self) {
        if self.panel == Panel::None || self.query.is_empty() {
            return;
        }
        self.query.clear();
        self.selection = 0;
        self.scroll = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.rows.len();
        if len == 0 {
            self.selection = 0;
            self.scroll = 0;
            return;
        }
        let step = delta.rem_euclid(len as isize) as usize;
        self.selection = (self.selection + step) % len;
        self.reveal();
    }

    pub fn selection(&self) -> usize {
        self.selection
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selection)
    }

    pub fn visible_selection(&self) -> Option<usize> {
        if self.selection < self.scroll || self.rows.is_empty() {
            return None;
        }
        let offset = self.selection - self.scroll;
        if offset < VISIBLE_ROWS {
            Some(offset)
        } else {
            None
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn visible_rows(&self) -> &[Row] {
        let end = self
            .scroll
            .saturating_add(VISIBLE_ROWS)
            .min(self.rows.len());
        let start = self.scroll.min(end);
        &self.rows[start..end]
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        if self.rows.is_empty() {
            self.selection = 0;
            self.scroll = 0;
            return;
        }
        if self.selection >= self.rows.len() {
            self.selection = self.rows.len() - 1;
        }
        self.reveal();
    }

    pub fn take_rows(&mut self) -> Vec<Row> {
        let rows = std::mem::take(&mut self.rows);
        self.selection = 0;
        self.scroll = 0;
        rows
    }

    pub fn set_status(&mut self, status: String) {
        if self.status == status {
            return;
        }
        self.status = status;
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn status_mut(&mut self) -> &mut String {
        &mut self.status
    }

    pub fn title(&self) -> &str {
        self.panel.title()
    }

    pub fn select_at(&mut self, visible_index: usize) -> Option<&Row> {
        if visible_index >= VISIBLE_ROWS {
            return None;
        }
        let index = self.scroll + visible_index;
        if index >= self.rows.len() {
            return None;
        }
        self.selection = index;
        self.reveal();
        self.rows.get(self.selection)
    }

    pub fn sidebar(&self) -> bool {
        self.sidebar
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar = !self.sidebar;
    }

    pub fn set_sidebar(&mut self, shown: bool) {
        self.sidebar = shown;
    }

    pub fn output(&self) -> bool {
        self.output
    }

    pub fn toggle_output(&mut self) {
        self.output = !self.output;
        if self.output {
            self.output_follow = true;
        }
    }

    pub fn set_output(&mut self, shown: bool) {
        if shown && !self.output {
            self.output_follow = true;
        }
        self.output = shown;
    }

    pub fn output_scroll(&self) -> usize {
        self.output_scroll
    }

    pub fn scroll_output(&mut self, delta: isize, total: usize) {
        let max = total.saturating_sub(self.output_rows);
        self.output_scroll = shift(self.output_scroll, delta, max);
        self.output_follow = self.output_scroll >= max;
    }

    pub fn output_follow(&self) -> bool {
        self.output_follow
    }

    pub fn follow_output(&mut self, total: usize) {
        let max = total.saturating_sub(self.output_rows);
        if self.output_follow || self.output_scroll > max {
            self.output_scroll = max;
        }
    }

    pub fn set_output_rows(&mut self, rows: usize) {
        self.output_rows = rows.max(1);
    }

    pub fn output_rows(&self) -> usize {
        self.output_rows
    }

    pub fn tree_scroll(&self) -> usize {
        self.tree_scroll
    }

    pub fn scroll_tree(&mut self, delta: isize, total: usize) {
        let max = total.saturating_sub(self.tree_rows);
        self.tree_scroll = shift(self.tree_scroll, delta, max);
    }

    pub fn set_tree_rows(&mut self, rows: usize) {
        self.tree_rows = rows.max(1);
    }

    pub fn tree_rows(&self) -> usize {
        self.tree_rows
    }

    pub fn sidebar_selection(&self) -> usize {
        self.sidebar_selection
    }

    pub fn set_sidebar_selection(&mut self, index: usize) {
        self.sidebar_selection = index;
    }

    pub fn reveal_tree(&mut self, index: usize, total: usize) {
        let max = total.saturating_sub(self.tree_rows);
        if index < self.tree_scroll {
            self.tree_scroll = index;
        } else if index >= self.tree_scroll + self.tree_rows {
            self.tree_scroll = index + 1 - self.tree_rows;
        }
        if self.tree_scroll > max {
            self.tree_scroll = max;
        }
    }

    fn remember_query(&mut self) {
        if self.panel == Panel::Search {
            self.search_query.clear();
            self.search_query.push_str(&self.query);
        }
    }

    fn reveal(&mut self) {
        let max = self.rows.len().saturating_sub(VISIBLE_ROWS);
        if self.selection < self.scroll {
            self.scroll = self.selection;
        } else if self.selection >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selection + 1 - VISIBLE_ROWS;
        }
        if self.scroll > max {
            self.scroll = max;
        }
    }
}

fn shift(offset: usize, delta: isize, max: usize) -> usize {
    let value = (offset as isize).saturating_add(delta);
    if value <= 0 {
        return 0;
    }
    (value as usize).min(max)
}

fn floor_boundary(text: &str, limit: usize) -> usize {
    if text.len() <= limit {
        return text.len();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Vec<Row> {
        (0..count)
            .map(|index| {
                Row::new(
                    format!("ligne {index}"),
                    format!("detail {index}"),
                    "src",
                    RowKind::File,
                    Some((
                        PathBuf::from(format!("/tmp/f{index}.rs")),
                        Cursor {
                            line: index,
                            column: 0,
                        },
                    )),
                )
            })
            .collect()
    }

    const ALL: [Panel; 9] = [
        Panel::None,
        Panel::QuickOpen,
        Panel::DocumentSymbols,
        Panel::WorkspaceSymbols,
        Panel::Search,
        Panel::Problems,
        Panel::References,
        Panel::Rename,
        Panel::Commands,
    ];

    #[test]
    fn etat_initial() {
        let overlay = Overlay::new();
        assert_eq!(overlay.panel(), Panel::None);
        assert!(!overlay.is_capturing_input());
        assert_eq!(overlay.query(), "");
        assert_eq!(overlay.title(), "");
        assert_eq!(overlay.status(), "");
        assert_eq!(overlay.scroll(), 0);
        assert_eq!(overlay.selection(), 0);
        assert!(overlay.rows().is_empty());
        assert!(overlay.visible_rows().is_empty());
        assert!(overlay.selected().is_none());
        assert!(overlay.sidebar());
        assert!(!overlay.output());
        assert_eq!(overlay.tree_scroll(), 0);
        assert_eq!(overlay.output_scroll(), 0);
        assert_eq!(overlay.sidebar_selection(), 0);
    }

    #[test]
    fn titres_sans_accent_et_en_minuscules() {
        for panel in ALL {
            let title = panel.title();
            assert!(title.is_ascii(), "titre non ascii pour {panel:?}");
            assert_eq!(title, title.to_lowercase());
            if panel != Panel::None {
                assert!(!title.is_empty());
            }
        }
    }

    #[test]
    fn ouverture_capture_la_saisie() {
        let mut overlay = Overlay::new();
        for panel in ALL {
            overlay.close();
            overlay.open(panel);
            assert_eq!(overlay.panel(), panel);
            assert_eq!(overlay.is_capturing_input(), panel != Panel::None);
            assert_eq!(overlay.title(), panel.title());
        }
    }

    #[test]
    fn ouvrir_none_equivaut_a_fermer() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.insert('a');
        overlay.open(Panel::None);
        assert_eq!(overlay.panel(), Panel::None);
        assert_eq!(overlay.query(), "");
        assert!(!overlay.is_capturing_input());
    }

    #[test]
    fn reouvrir_le_meme_panneau_ne_reinitialise_rien() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.insert('m');
        overlay.set_rows(rows(30));
        overlay.move_selection(5);
        overlay.open(Panel::QuickOpen);
        assert_eq!(overlay.query(), "m");
        assert_eq!(overlay.rows().len(), 30);
        assert_eq!(overlay.selection(), 5);
    }

    #[test]
    fn changer_de_panneau_reinitialise_lignes_et_requete() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.insert('x');
        overlay.set_rows(rows(30));
        overlay.move_selection(20);
        overlay.set_status(String::from("21 sur 30"));
        overlay.open(Panel::Commands);
        assert_eq!(overlay.query(), "");
        assert!(overlay.rows().is_empty());
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        assert_eq!(overlay.status(), "");
    }

    #[test]
    fn la_recherche_conserve_sa_requete() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.insert('f');
        overlay.insert('o');
        overlay.insert('o');
        overlay.close();
        assert_eq!(overlay.query(), "");
        overlay.open(Panel::Search);
        assert_eq!(overlay.query(), "foo");
        overlay.open(Panel::QuickOpen);
        assert_eq!(overlay.query(), "");
        overlay.open(Panel::Search);
        assert_eq!(overlay.query(), "foo");
    }

    #[test]
    fn les_autres_panneaux_repartent_de_zero() {
        let mut overlay = Overlay::new();
        for panel in [
            Panel::QuickOpen,
            Panel::DocumentSymbols,
            Panel::WorkspaceSymbols,
            Panel::Problems,
            Panel::References,
            Panel::Rename,
            Panel::Commands,
        ] {
            overlay.open(panel);
            overlay.insert('z');
            overlay.close();
            overlay.open(panel);
            assert_eq!(overlay.query(), "", "requete gardee pour {panel:?}");
        }
    }

    #[test]
    fn renommer_est_prerempli() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Rename);
        overlay.set_query(String::from("ancien_nom"));
        assert_eq!(overlay.query(), "ancien_nom");
        assert_eq!(overlay.selection(), 0);
        overlay.backspace();
        assert_eq!(overlay.query(), "ancien_no");
        overlay.open_with(Panel::Rename, String::from("autre"));
        assert_eq!(overlay.query(), "autre");
        assert_eq!(overlay.panel(), Panel::Rename);
    }

    #[test]
    fn open_with_ouvre_et_prerempli() {
        let mut overlay = Overlay::new();
        overlay.open_with(Panel::WorkspaceSymbols, String::from("Serveur"));
        assert_eq!(overlay.panel(), Panel::WorkspaceSymbols);
        assert_eq!(overlay.query(), "Serveur");
    }

    #[test]
    fn saisie_ignoree_quand_aucun_panneau() {
        let mut overlay = Overlay::new();
        overlay.insert('a');
        overlay.backspace();
        overlay.clear_query();
        overlay.move_selection(3);
        assert_eq!(overlay.query(), "");
        assert_eq!(overlay.selection(), 0);
        assert!(!overlay.is_capturing_input());
    }

    #[test]
    fn la_frappe_ne_fuit_jamais_vers_le_tampon() {
        let mut overlay = Overlay::new();
        let mut buffer = String::new();
        fn tape(overlay: &mut Overlay, buffer: &mut String, ch: char) {
            if overlay.is_capturing_input() {
                overlay.insert(ch);
            } else {
                buffer.push(ch);
            }
        }
        tape(&mut overlay, &mut buffer, 'a');
        overlay.open(Panel::Search);
        for ch in "main.rs".chars() {
            tape(&mut overlay, &mut buffer, ch);
        }
        assert_eq!(buffer, "a");
        assert_eq!(overlay.query(), "main.rs");
        overlay.close();
        tape(&mut overlay, &mut buffer, 'b');
        assert_eq!(buffer, "ab");
        for panel in ALL {
            if panel == Panel::None {
                continue;
            }
            overlay.open(panel);
            for ch in "xyz".chars() {
                tape(&mut overlay, &mut buffer, ch);
            }
            overlay.close();
        }
        assert_eq!(buffer, "ab");
    }

    #[test]
    fn caracteres_de_controle_refuses() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        for ch in ['\n', '\r', '\t', '\u{1b}', '\u{7f}', '\u{0}'] {
            overlay.insert(ch);
        }
        assert_eq!(overlay.query(), "");
    }

    #[test]
    fn requete_plafonnee() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        for _ in 0..MAX_QUERY * 2 {
            overlay.insert('a');
        }
        assert_eq!(overlay.query().len(), MAX_QUERY);
        overlay.insert('e');
        assert_eq!(overlay.query().len(), MAX_QUERY);
    }

    #[test]
    fn set_query_tronque_sur_une_frontiere_de_caractere() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        let long = "e\u{301}".repeat(MAX_QUERY);
        overlay.set_query(long);
        assert!(overlay.query().len() <= MAX_QUERY);
        assert!(overlay.query().is_char_boundary(overlay.query().len()));
        let count = overlay.query().chars().count();
        assert!(count > 0);
    }

    #[test]
    fn effacement_par_caractere_pas_par_octet() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Rename);
        overlay.set_query(String::from("cafe\u{301}"));
        overlay.backspace();
        assert_eq!(overlay.query(), "cafe");
        overlay.insert('\u{1f600}');
        assert_eq!(overlay.query(), "cafe\u{1f600}");
        overlay.backspace();
        assert_eq!(overlay.query(), "cafe");
        for _ in 0..10 {
            overlay.backspace();
        }
        assert_eq!(overlay.query(), "");
    }

    #[test]
    fn la_frappe_remet_la_selection_en_haut() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(40));
        overlay.move_selection(30);
        assert_eq!(overlay.selection(), 30);
        assert!(overlay.scroll() > 0);
        overlay.insert('a');
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        overlay.move_selection(30);
        overlay.backspace();
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        overlay.insert('b');
        overlay.move_selection(30);
        overlay.clear_query();
        assert_eq!(overlay.query(), "");
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        overlay.move_selection(30);
        overlay.clear_query();
        assert_eq!(overlay.selection(), 30);
    }

    #[test]
    fn navigation_qui_boucle() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Commands);
        overlay.set_rows(rows(5));
        overlay.move_selection(-1);
        assert_eq!(overlay.selection(), 4);
        overlay.move_selection(1);
        assert_eq!(overlay.selection(), 0);
        overlay.move_selection(4);
        assert_eq!(overlay.selection(), 4);
        overlay.move_selection(2);
        assert_eq!(overlay.selection(), 1);
        overlay.move_selection(-3);
        assert_eq!(overlay.selection(), 3);
    }

    #[test]
    fn navigation_avec_un_delta_enorme() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Commands);
        overlay.set_rows(rows(7));
        overlay.move_selection(7 * 1000 + 3);
        assert_eq!(overlay.selection(), 3);
        overlay.move_selection(-7 * 1000 - 1);
        assert_eq!(overlay.selection(), 2);
        overlay.move_selection(isize::MAX);
        assert!(overlay.selection() < 7);
        overlay.move_selection(isize::MIN);
        assert!(overlay.selection() < 7);
        assert!(overlay.scroll() == 0);
    }

    #[test]
    fn navigation_sur_liste_vide() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Problems);
        overlay.move_selection(1);
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        assert!(overlay.selected().is_none());
        assert!(overlay.visible_rows().is_empty());
        assert!(overlay.visible_selection().is_none());
        overlay.move_selection(-1);
        assert_eq!(overlay.selection(), 0);
    }

    #[test]
    fn seize_lignes_visibles_au_maximum() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(9));
        assert_eq!(overlay.visible_rows().len(), 9);
        overlay.set_rows(rows(VISIBLE_ROWS));
        assert_eq!(overlay.visible_rows().len(), VISIBLE_ROWS);
        overlay.set_rows(rows(340));
        assert_eq!(overlay.visible_rows().len(), VISIBLE_ROWS);
    }

    #[test]
    fn le_defilement_suit_la_selection_vers_le_bas() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(40));
        for step in 0..VISIBLE_ROWS {
            overlay.move_selection(1);
            assert_eq!(overlay.selection(), step + 1);
            let expected = (step + 1 + 1).saturating_sub(VISIBLE_ROWS);
            assert_eq!(overlay.scroll(), expected);
        }
        assert_eq!(overlay.visible_selection(), Some(VISIBLE_ROWS - 1));
        assert_eq!(overlay.visible_rows()[VISIBLE_ROWS - 1].label, "ligne 16");
    }

    #[test]
    fn le_defilement_suit_la_selection_vers_le_haut() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(40));
        overlay.move_selection(30);
        assert_eq!(overlay.scroll(), 30 + 1 - VISIBLE_ROWS);
        overlay.move_selection(-20);
        assert_eq!(overlay.selection(), 10);
        assert_eq!(overlay.scroll(), 10);
        assert_eq!(overlay.visible_selection(), Some(0));
    }

    #[test]
    fn le_bouclage_recadre_le_defilement() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(40));
        overlay.move_selection(-1);
        assert_eq!(overlay.selection(), 39);
        assert_eq!(overlay.scroll(), 40 - VISIBLE_ROWS);
        assert_eq!(overlay.visible_selection(), Some(VISIBLE_ROWS - 1));
        overlay.move_selection(1);
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        assert_eq!(overlay.visible_selection(), Some(0));
    }

    #[test]
    fn fenetre_visible_alignee_sur_le_defilement() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.set_rows(rows(100));
        overlay.move_selection(50);
        let scroll = overlay.scroll();
        let visible = overlay.visible_rows();
        assert_eq!(visible.len(), VISIBLE_ROWS);
        assert_eq!(visible[0].label, format!("ligne {scroll}"));
        assert_eq!(
            visible[VISIBLE_ROWS - 1].label,
            format!("ligne {}", scroll + VISIBLE_ROWS - 1)
        );
        let offset = overlay.visible_selection().expect("selection visible");
        assert_eq!(visible[offset].label, "ligne 50");
    }

    #[test]
    fn selection_par_clic() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::References);
        overlay.set_rows(rows(40));
        overlay.move_selection(20);
        let scroll = overlay.scroll();
        let label = overlay
            .select_at(3)
            .map(|row| row.label.clone())
            .expect("ligne cliquee");
        assert_eq!(label, format!("ligne {}", scroll + 3));
        assert_eq!(overlay.selection(), scroll + 3);
        assert!(overlay.select_at(VISIBLE_ROWS).is_none());
        overlay.set_rows(rows(2));
        assert!(overlay.select_at(0).is_some());
        assert!(overlay.select_at(2).is_none());
    }

    #[test]
    fn set_rows_conserve_la_selection_quand_elle_tient() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.set_rows(rows(100));
        overlay.move_selection(40);
        let scroll = overlay.scroll();
        overlay.set_rows(rows(200));
        assert_eq!(overlay.selection(), 40);
        assert_eq!(overlay.scroll(), scroll);
    }

    #[test]
    fn set_rows_replie_la_selection_qui_deborde() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.set_rows(rows(100));
        overlay.move_selection(80);
        overlay.set_rows(rows(10));
        assert_eq!(overlay.selection(), 9);
        assert_eq!(overlay.scroll(), 0);
        assert_eq!(overlay.visible_rows().len(), 10);
        overlay.set_rows(Vec::new());
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        assert!(overlay.selected().is_none());
    }

    #[test]
    fn take_rows_rend_le_tampon() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        overlay.set_rows(rows(20));
        overlay.move_selection(18);
        let mut recycled = overlay.take_rows();
        assert_eq!(recycled.len(), 20);
        assert!(overlay.rows().is_empty());
        assert_eq!(overlay.selection(), 0);
        assert_eq!(overlay.scroll(), 0);
        recycled.clear();
        recycled.extend(rows(3));
        overlay.set_rows(recycled);
        assert_eq!(overlay.rows().len(), 3);
    }

    #[test]
    fn cible_de_la_ligne_selectionnee() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Problems);
        overlay.set_rows(rows(4));
        overlay.move_selection(2);
        let row = overlay.selected().expect("ligne selectionnee");
        assert_eq!(row.tag, "src");
        assert_eq!(row.kind, RowKind::File);
        let (path, cursor) = row.target.as_ref().expect("cible");
        assert_eq!(path, &PathBuf::from("/tmp/f2.rs"));
        assert_eq!(cursor.line, 2);
        assert_eq!(cursor.column, 0);
    }

    #[test]
    fn ligne_sans_cible() {
        let row = Row::plain(String::from("aucun resultat"));
        assert!(row.target.is_none());
        assert_eq!(row.detail, "");
        assert_eq!(row.kind, RowKind::Plain);
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.set_rows(vec![row]);
        assert!(overlay.selected().expect("ligne").target.is_none());
    }

    #[test]
    fn statut_du_panneau() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::Search);
        overlay.set_status(String::from("12 sur 340"));
        assert_eq!(overlay.status(), "12 sur 340");
        overlay.set_status(String::from("12 sur 340"));
        assert_eq!(overlay.status(), "12 sur 340");
        overlay.status_mut().clear();
        assert_eq!(overlay.status(), "");
        overlay.set_status(String::from("recherche en cours"));
        overlay.close();
        assert_eq!(overlay.status(), "");
    }

    #[test]
    fn panneaux_qui_prennent_une_requete() {
        assert!(Panel::QuickOpen.takes_query());
        assert!(Panel::Rename.takes_query());
        assert!(Panel::Commands.takes_query());
        assert!(!Panel::Problems.takes_query());
        assert!(!Panel::References.takes_query());
        assert!(!Panel::None.takes_query());
    }

    #[test]
    fn bascule_de_l_arbre_lateral() {
        let mut overlay = Overlay::new();
        assert!(overlay.sidebar());
        overlay.toggle_sidebar();
        assert!(!overlay.sidebar());
        overlay.toggle_sidebar();
        assert!(overlay.sidebar());
        overlay.set_sidebar(false);
        assert!(!overlay.sidebar());
    }

    #[test]
    fn bascule_du_panneau_de_sortie() {
        let mut overlay = Overlay::new();
        assert!(!overlay.output());
        overlay.toggle_output();
        assert!(overlay.output());
        assert!(overlay.output_follow());
        overlay.toggle_output();
        assert!(!overlay.output());
        overlay.set_output(true);
        assert!(overlay.output());
    }

    #[test]
    fn defilement_de_la_sortie_borne() {
        let mut overlay = Overlay::new();
        overlay.set_output(true);
        overlay.scroll_output(-5, 100);
        assert_eq!(overlay.output_scroll(), 0);
        overlay.scroll_output(10, 100);
        assert_eq!(overlay.output_scroll(), 10);
        overlay.scroll_output(1000, 100);
        assert_eq!(overlay.output_scroll(), 100 - OUTPUT_ROWS);
        overlay.scroll_output(5, 4);
        assert_eq!(overlay.output_scroll(), 0);
        overlay.scroll_output(isize::MIN, 100);
        assert_eq!(overlay.output_scroll(), 0);
        overlay.scroll_output(isize::MAX, 100);
        assert_eq!(overlay.output_scroll(), 100 - OUTPUT_ROWS);
    }

    #[test]
    fn la_sortie_reste_ancree_en_bas() {
        let mut overlay = Overlay::new();
        overlay.set_output(true);
        assert!(overlay.output_follow());
        overlay.follow_output(50);
        assert_eq!(overlay.output_scroll(), 50 - OUTPUT_ROWS);
        overlay.follow_output(80);
        assert_eq!(overlay.output_scroll(), 80 - OUTPUT_ROWS);
        overlay.scroll_output(-20, 80);
        assert!(!overlay.output_follow());
        let parked = overlay.output_scroll();
        overlay.follow_output(200);
        assert_eq!(overlay.output_scroll(), parked);
        overlay.scroll_output(10_000, 200);
        assert!(overlay.output_follow());
        overlay.follow_output(400);
        assert_eq!(overlay.output_scroll(), 400 - OUTPUT_ROWS);
    }

    #[test]
    fn la_sortie_se_replie_quand_les_lignes_disparaissent() {
        let mut overlay = Overlay::new();
        overlay.set_output(true);
        overlay.scroll_output(40, 200);
        assert!(!overlay.output_follow());
        assert_eq!(overlay.output_scroll(), 40);
        overlay.follow_output(20);
        assert_eq!(overlay.output_scroll(), 20 - OUTPUT_ROWS);
    }

    #[test]
    fn hauteur_de_sortie_configurable() {
        let mut overlay = Overlay::new();
        overlay.set_output_rows(0);
        assert_eq!(overlay.output_rows(), 1);
        overlay.set_output_rows(30);
        assert_eq!(overlay.output_rows(), 30);
        overlay.scroll_output(1000, 100);
        assert_eq!(overlay.output_scroll(), 70);
    }

    #[test]
    fn defilement_de_l_arbre_borne() {
        let mut overlay = Overlay::new();
        overlay.scroll_tree(-3, 100);
        assert_eq!(overlay.tree_scroll(), 0);
        overlay.scroll_tree(5, 100);
        assert_eq!(overlay.tree_scroll(), 5);
        overlay.scroll_tree(1000, 100);
        assert_eq!(overlay.tree_scroll(), 100 - TREE_ROWS);
        overlay.scroll_tree(10, 10);
        assert_eq!(overlay.tree_scroll(), 0);
        overlay.set_tree_rows(4);
        overlay.scroll_tree(1000, 10);
        assert_eq!(overlay.tree_scroll(), 6);
    }

    #[test]
    fn l_arbre_revele_la_ligne_demandee() {
        let mut overlay = Overlay::new();
        overlay.set_tree_rows(10);
        overlay.reveal_tree(4, 100);
        assert_eq!(overlay.tree_scroll(), 0);
        overlay.reveal_tree(30, 100);
        assert_eq!(overlay.tree_scroll(), 21);
        overlay.reveal_tree(2, 100);
        assert_eq!(overlay.tree_scroll(), 2);
        overlay.reveal_tree(99, 100);
        assert_eq!(overlay.tree_scroll(), 90);
        overlay.reveal_tree(5, 6);
        assert_eq!(overlay.tree_scroll(), 0);
    }

    #[test]
    fn selection_de_l_arbre() {
        let mut overlay = Overlay::new();
        overlay.set_sidebar_selection(7);
        assert_eq!(overlay.sidebar_selection(), 7);
        overlay.open(Panel::QuickOpen);
        overlay.close();
        assert_eq!(overlay.sidebar_selection(), 7);
    }

    #[test]
    fn les_panneaux_n_affectent_pas_l_arbre_ni_la_sortie() {
        let mut overlay = Overlay::new();
        overlay.set_output(true);
        overlay.scroll_output(6, 100);
        overlay.scroll_tree(9, 100);
        overlay.open(Panel::Commands);
        overlay.set_rows(rows(20));
        overlay.move_selection(19);
        overlay.close();
        assert_eq!(overlay.output_scroll(), 6);
        assert_eq!(overlay.tree_scroll(), 9);
        assert!(overlay.output());
        assert!(overlay.sidebar());
    }

    #[test]
    fn echap_ferme_puis_rend_le_clavier_au_tampon() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::WorkspaceSymbols);
        overlay.set_rows(rows(30));
        overlay.move_selection(25);
        overlay.insert('S');
        assert!(overlay.is_capturing_input());
        overlay.close();
        assert!(!overlay.is_capturing_input());
        assert_eq!(overlay.panel(), Panel::None);
        assert!(overlay.rows().is_empty());
        assert!(overlay.selected().is_none());
        overlay.close();
        assert_eq!(overlay.panel(), Panel::None);
    }

    #[test]
    fn cycle_complet_d_ouverture_rapide() {
        let mut overlay = Overlay::new();
        overlay.open(Panel::QuickOpen);
        for ch in "ove".chars() {
            overlay.insert(ch);
        }
        overlay.set_rows(rows(3));
        overlay.set_status(String::from("1 sur 3"));
        overlay.move_selection(1);
        let chosen = overlay
            .selected()
            .and_then(|row| row.target.clone())
            .expect("cible");
        assert_eq!(chosen.0, PathBuf::from("/tmp/f1.rs"));
        overlay.close();
        assert_eq!(overlay.panel(), Panel::None);
        assert_eq!(overlay.query(), "");
    }
}
