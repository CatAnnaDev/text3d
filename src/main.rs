mod atlas;
mod camera;
mod complete;
mod export;
mod extrude;
mod font;
mod history;
mod layout;
mod pick;
mod render;
mod syntax;
mod text;

#[cfg(test)]
mod tests;

use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use camera::Camera;
use complete::{Completion, MIN_PREFIX};
use font::Font;
use layout::LineLayout;
use render::{FindBarView, Renderer};
use syntax::{Highlighter, Language};
use text::{Cursor, TextBuffer};

const EDGE_PADDING: f32 = 1.2;
const STATUS_LIFETIME: f32 = 4.0;
const DRAG_THRESHOLD: f32 = 4.0;
const CLICK_SLACK: f32 = 5.0;
const MULTI_CLICK_DELAY: f32 = 0.4;
const LOD_TRAVEL_RATIO: f32 = 0.08;
const MATCH_PAD_COLUMNS: f32 = 16.0;
const MATCH_PAD_LINES: f32 = 6.0;
const DEMO: &str = "\
bloc-notes en trois dimensions
------------------------------

chaque lettre est un maillage extrude et
biseaute, tessele depuis les contours de la
fonte puis rendu en instancie sur le GPU,
avec ombres portees et niveaux de detail.

ouvre un .rs ou un .cs : tree-sitter colore
la syntaxe et la completion propose les
symboles du fichier, les mots-cles et les
mots du tampon, classes en fuzzy.

  souris
  clic gauche         poser le curseur
  double clic         selectionner le mot
  triple clic         selectionner la ligne
  shift + glisser     selectionner a la souris
  glisser gauche      tourner autour du texte
  glisser droit       translater
  molette             avancer ou reculer
  option + fleches    tourner au clavier

  edition
  shift + fleches     etendre la selection
  cmd + alt + fleches se deplacer par mot
  cmd + A             tout selectionner
  cmd + C / X / V     copier, couper, coller
  cmd + Z             annuler
  cmd + shift + Z     refaire
  alt + retour        effacer le mot a gauche
  cmd + S             enregistrer
  tab / ctrl + espace ouvrir la completion
  cmd + gauche/droite debut ou fin de ligne
  cmd + haut/bas      haut ou bas du document

  recherche
  cmd + F             ouvrir la barre
  entree              occurrence suivante
  shift + entree      occurrence precedente
  tab                 champ de remplacement
  cmd + entree        tout remplacer
  cmd + G             suivante sans la barre
  echap               fermer

  vue
  F1 recadrer         F2 onduler
  F3 grille           F4 ombres
  F5 fonte suivante   F6 biseau
  F7 relief par indentation

  export
  cmd + E             maillage obj
  cmd + shift + E     maillage glb
  cmd + P             capture png

tape quelque chose, la ligne courante
se souleve vers toi et prend la couleur
chaude. tourne autour pour voir l epaisseur
des glyphes.

0123456789 accents: e a u i o c
{ } [ ] ( ) < > / \\ | & % $ # @ ! ?
";

struct Pointer {
    orbiting: bool,
    panning: bool,
    selecting: bool,
    pressed: bool,
    moved: bool,
    handled: bool,
    clicks: u32,
    last: (f32, f32),
    press_at: (f32, f32),
    click_at: (f32, f32),
    click_time: Option<Instant>,
}

impl Pointer {
    fn new() -> Pointer {
        Pointer {
            orbiting: false,
            panning: false,
            selecting: false,
            pressed: false,
            moved: false,
            handled: false,
            clicks: 0,
            last: (0.0, 0.0),
            press_at: (0.0, 0.0),
            click_at: (0.0, 0.0),
            click_time: None,
        }
    }

    fn release_all(&mut self) {
        self.pressed = false;
        self.orbiting = false;
        self.panning = false;
        self.selecting = false;
    }
}

#[derive(Default)]
struct FindBar {
    open: bool,
    replacing: bool,
    query: String,
    replacement: String,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    font: Font,
    text: TextBuffer,
    camera: Camera,
    pointer: Pointer,
    modifiers: ModifiersState,
    start: Instant,
    last_frame: Instant,
    needs_rebuild: bool,
    needs_popup: bool,
    needs_find_bar: bool,
    language: Option<Language>,
    highlighter: Option<Highlighter>,
    completion: Completion,
    find: FindBar,
    focus_layout: LineLayout,
    focus_line: usize,
    focus_version: u64,
    focus_locked: bool,
    pick_layout: LineLayout,
    status_since: Option<Instant>,
    scroll: (f32, f32),
    viewport: (f32, f32),
    title: String,
    title_scratch: String,
    wave: bool,
    grid: bool,
    rebuild_eye: Vec3,
    rebuild_distance: f32,
    shaded_version: u64,
    shaded_start: usize,
    shaded_end: usize,
}

impl App {
    fn new(font: Font, text: TextBuffer) -> Self {
        let language = Language::detect(text.path.as_deref());
        let highlighter = language.and_then(Highlighter::new);
        App {
            language,
            highlighter,
            completion: Completion::new(),
            window: None,
            renderer: None,
            font,
            text,
            camera: Camera::new(),
            pointer: Pointer::new(),
            modifiers: ModifiersState::empty(),
            start: Instant::now(),
            last_frame: Instant::now(),
            needs_rebuild: true,
            needs_popup: true,
            needs_find_bar: true,
            find: FindBar::default(),
            focus_layout: LineLayout::default(),
            focus_line: 0,
            focus_version: 0,
            focus_locked: false,
            pick_layout: LineLayout::default(),
            status_since: None,
            scroll: (0.0, 0.0),
            viewport: (1280.0, 800.0),
            title: String::new(),
            title_scratch: String::new(),
            wave: true,
            grid: true,
            rebuild_eye: Vec3::ZERO,
            rebuild_distance: 1.0,
            shaded_version: 0,
            shaded_start: 0,
            shaded_end: 0,
        }
    }

    fn set_status(&mut self, message: String) {
        self.text.status = message;
        self.status_since = Some(Instant::now());
    }

    fn refresh_view(&mut self) {
        self.needs_rebuild = true;
        self.focus_locked = false;
    }

    fn update_completion(&mut self, force: bool) {
        self.text.sync();
        if !force && self.language.is_none() && !self.completion.active {
            self.completion.dismiss();
            self.needs_popup = true;
            return;
        }
        let min_prefix = if force { 1 } else { MIN_PREFIX };
        self.completion.refresh(
            &self.text,
            self.language,
            self.highlighter.as_ref(),
            min_prefix,
        );
        self.needs_popup = true;
    }

    fn accept_completion(&mut self) {
        let Some(candidate) = self.completion.selection() else {
            return;
        };
        let replacement = candidate.text.clone();
        for _ in 0..self.completion.prefix_chars {
            self.text.backspace();
        }
        self.text.insert_str(&replacement);
        self.completion.dismiss();
        self.refresh_view();
        self.needs_popup = true;
    }

    fn close_popup(&mut self) {
        self.completion.dismiss();
        self.needs_popup = true;
    }

    fn aspect(&self) -> f32 {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.aspect())
            .unwrap_or(1.6)
    }

    fn cursor_x(&mut self) -> f32 {
        let version = self.text.version();
        if self.focus_version != version || self.focus_line != self.text.cursor_line {
            self.focus_version = version;
            self.focus_line = self.text.cursor_line;
            let line = self.text.lines[self.text.cursor_line].as_str();
            self.focus_layout.build(&self.font, line);
        }
        self.focus_layout.x_of_column(self.text.cursor_col)
    }

    fn update_focus(&mut self) {
        if self.focus_locked {
            self.camera
                .set_focus(Vec3::new(self.scroll.0, self.scroll.1, 0.0));
            return;
        }
        let (half_width, half_height) = self.camera.half_extent(self.aspect());
        let cursor_x = self.cursor_x();
        let cursor_y = -(self.text.cursor_line as f32) * self.font.line_height();

        let margin_x = half_width * 0.15;
        self.scroll.0 = keep_within(self.scroll.0, cursor_x, half_width - margin_x)
            .max(half_width * 0.90 - EDGE_PADDING);

        let margin_y = half_height * 0.18;
        let reach_y = half_height - margin_y;
        self.scroll.1 = keep_within(self.scroll.1, cursor_y, reach_y);
        if self.completion.active {
            let rows = self.completion.items.len().min(complete::VISIBLE_ROWS) as f32;
            let popup_bottom = cursor_y - (rows + 0.6) * self.font.line_height();
            self.scroll.1 = keep_within(self.scroll.1, popup_bottom, reach_y);
        }
        self.scroll.1 = self.scroll.1.min(EDGE_PADDING - half_height * 0.90);

        self.camera
            .set_focus(Vec3::new(self.scroll.0, self.scroll.1, 0.0));
    }

    fn frame_on(&mut self, min: Vec3, max: Vec3) {
        let aspect = self.aspect();
        self.camera.frame_selection(min, max, aspect);
        self.scroll = (self.camera.focus.x, self.camera.focus.y);
        self.focus_locked = true;
    }

    fn refresh_title(&mut self) {
        let stats = self.renderer.as_ref().map(|renderer| renderer.stats());
        self.title_scratch.clear();
        self.title_scratch.push_str("text3d  |  ");
        match self.text.path.as_deref().and_then(|path| path.file_name()) {
            Some(name) => self.title_scratch.push_str(&name.to_string_lossy()),
            None => self.title_scratch.push_str("sans titre"),
        }
        if self.text.modified {
            self.title_scratch.push_str(" *");
        }
        let _ = write!(
            self.title_scratch,
            "  |  {}:{}",
            self.text.cursor_line + 1,
            self.text.cursor_col + 1
        );
        if let Some(language) = self.language {
            self.title_scratch.push_str("  |  ");
            self.title_scratch.push_str(language.label());
        }
        self.title_scratch.push_str("  |  ");
        self.title_scratch.push_str(self.font.name());
        if let Some(stats) = stats {
            self.title_scratch.push_str("  |  ");
            write_compact(&mut self.title_scratch, stats.instances);
            self.title_scratch.push_str(" glyphes  |  ");
            write_compact(&mut self.title_scratch, stats.triangles);
            self.title_scratch.push_str(" tri");
        }
        if !self.text.status.is_empty() {
            self.title_scratch.push_str("  |  ");
            self.title_scratch.push_str(&self.text.status);
        }
        if self.title_scratch != self.title {
            self.title.clear();
            self.title.push_str(&self.title_scratch);
            if let Some(window) = &self.window {
                window.set_title(&self.title);
            }
        }
    }

    fn pick_at(&mut self, x: f32, y: f32) -> Option<pick::Hit> {
        let aspect = self.renderer.as_ref()?.aspect();
        pick::hit_text(
            &self.camera,
            aspect,
            self.viewport.0,
            self.viewport.1,
            x,
            y,
            &self.text,
            &self.font,
            &mut self.pick_layout,
        )
    }

    fn press_left(&mut self) {
        let now = Instant::now();
        let at = self.pointer.last;
        let near = (at.0 - self.pointer.click_at.0).hypot(at.1 - self.pointer.click_at.1)
            <= CLICK_SLACK;
        let quick = self
            .pointer
            .click_time
            .is_some_and(|since| (now - since).as_secs_f32() <= MULTI_CLICK_DELAY);
        self.pointer.clicks = if near && quick { self.pointer.clicks % 3 + 1 } else { 1 };
        self.pointer.click_time = Some(now);
        self.pointer.click_at = at;
        self.pointer.press_at = at;
        self.pointer.pressed = true;
        self.pointer.moved = false;
        self.pointer.orbiting = false;
        self.pointer.handled = false;
        self.pointer.selecting = self.modifiers.shift_key();

        let clicks = self.pointer.clicks;
        if clicks == 1 && !self.pointer.selecting {
            return;
        }
        let Some(hit) = self.pick_at(at.0, at.1) else {
            return;
        };
        let cursor = Cursor { line: hit.line, column: hit.column };
        match clicks {
            2 => self.text.select_word_at(cursor),
            3 => self.text.select_line_at(hit.line),
            _ => self.text.set_cursor(cursor, false),
        }
        self.pointer.handled = true;
        self.refresh_view();
        self.close_popup();
    }

    fn release_left(&mut self) {
        let placing = self.pointer.pressed && !self.pointer.moved && !self.pointer.handled;
        self.pointer.pressed = false;
        self.pointer.orbiting = false;
        self.pointer.selecting = false;
        if !placing {
            return;
        }
        let at = self.pointer.last;
        let Some(hit) = self.pick_at(at.0, at.1) else {
            return;
        };
        self.text
            .set_cursor(Cursor { line: hit.line, column: hit.column }, false);
        self.refresh_view();
        self.close_popup();
    }

    fn cursor_moved(&mut self, x: f32, y: f32) {
        let delta = (x - self.pointer.last.0, y - self.pointer.last.1);
        self.pointer.last = (x, y);

        if self.pointer.selecting {
            if let Some(hit) = self.pick_at(x, y) {
                self.text
                    .set_cursor(Cursor { line: hit.line, column: hit.column }, true);
                self.refresh_view();
            }
            return;
        }

        if self.pointer.pressed && !self.pointer.moved {
            let travel = (x - self.pointer.press_at.0).hypot(y - self.pointer.press_at.1);
            if travel > DRAG_THRESHOLD {
                self.pointer.moved = true;
                self.pointer.orbiting = true;
            }
        }

        if self.pointer.orbiting {
            self.camera.orbit(delta.0, delta.1);
        } else if self.pointer.panning {
            let height = self
                .renderer
                .as_ref()
                .map(|renderer| renderer.viewport_height())
                .unwrap_or(self.viewport.1);
            self.camera.pan_by(delta.0, delta.1, height);
        }
    }

    fn select_everything(&mut self) {
        self.text.select_all();
        let advance = self.font.advance();
        let line_height = self.font.line_height();
        let mut widest = 0;
        for line in &self.text.lines {
            let count = line.chars().count();
            if count > widest {
                widest = count;
            }
        }
        let last = (self.text.line_count().max(1) - 1) as f32;
        let min = Vec3::new(-advance, -last * line_height - line_height, 0.0);
        let max = Vec3::new(widest as f32 * advance + advance, line_height, 0.0);
        self.refresh_view();
        self.close_popup();
        self.frame_on(min, max);
        self.set_status(String::from("tout selectionne"));
    }

    fn copy(&mut self) {
        match self.text.selected_text() {
            Some(selected) => {
                clipboard_write(&selected);
                self.set_status(String::from("selection copiee"));
            }
            None => {
                clipboard_write(self.text.current_line());
                self.set_status(String::from("ligne copiee"));
            }
        }
    }

    fn cut(&mut self) {
        match self.text.selected_text() {
            Some(selected) => {
                clipboard_write(&selected);
                self.text.delete_selection();
                self.set_status(String::from("selection coupee"));
            }
            None => {
                clipboard_write(self.text.current_line());
                self.text.delete_line();
                self.set_status(String::from("ligne coupee"));
            }
        }
        self.refresh_view();
        self.close_popup();
    }

    fn paste(&mut self) {
        let Some(clip) = clipboard_read() else {
            self.set_status(String::from("presse-papier vide"));
            return;
        };
        self.text.insert_text(&clip);
        self.refresh_view();
        self.close_popup();
    }

    fn undo_edit(&mut self) {
        if self.text.undo() {
            self.refresh_view();
            self.close_popup();
        } else {
            self.set_status(String::from("rien a annuler"));
        }
    }

    fn redo_edit(&mut self) {
        if self.text.redo() {
            self.refresh_view();
            self.close_popup();
        } else {
            self.set_status(String::from("rien a refaire"));
        }
    }

    fn open_find(&mut self) {
        self.find.open = true;
        self.find.replacing = false;
        self.close_popup();
        if let Some(selected) = self.text.selected_text()
            && !selected.contains('\n')
        {
            self.find.query.clear();
            self.find.query.push_str(&selected);
        }
        if self.find.query.is_empty() {
            self.needs_find_bar = true;
            self.needs_rebuild = true;
        } else {
            self.run_search();
        }
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.replacing = false;
        self.text.clear_matches();
        self.needs_find_bar = true;
        self.refresh_view();
    }

    fn run_search(&mut self) {
        self.text.find_all(&self.find.query, false);
        if self.text.focus_match(true) {
            self.sync_to_match();
        }
        self.needs_rebuild = true;
        self.needs_find_bar = true;
    }

    fn sync_to_match(&mut self) {
        let Some(index) = self.text.current_match() else {
            return;
        };
        let Some(found) = self.text.matches().get(index).copied() else {
            return;
        };
        self.text
            .set_cursor(Cursor { line: found.line, column: found.end }, false);
        self.text
            .set_cursor(Cursor { line: found.line, column: found.start }, true);
        self.needs_rebuild = true;
        self.focus_locked = false;

        let line_height = self.font.line_height();
        let y = -(found.line as f32) * line_height;
        self.focus_version = self.text.version();
        self.focus_line = found.line;
        let line = self.text.lines[found.line].as_str();
        self.focus_layout.build(&self.font, line);
        let left = self.focus_layout.x_of_column(found.start);
        let right = self.focus_layout.x_of_column(found.end);

        let (half_width, half_height) = self.camera.half_extent(self.aspect());
        let center_x = (left + right) * 0.5;
        let away = (center_x - self.camera.target.x).abs() > half_width * 0.8
            || (y - self.camera.target.y).abs() > half_height * 0.8;
        if !away {
            return;
        }
        let pad_x = self.font.advance() * MATCH_PAD_COLUMNS;
        let pad_y = line_height * MATCH_PAD_LINES;
        let min = Vec3::new(left - pad_x, y - line_height - pad_y, 0.0);
        let max = Vec3::new(right + pad_x, y + pad_y, 0.0);
        self.frame_on(min, max);
    }

    fn step_match(&mut self, forward: bool) {
        if self.find.query.is_empty() {
            self.set_status(String::from("aucune recherche"));
            return;
        }
        if self.text.matches().is_empty() {
            self.text.find_all(&self.find.query, false);
        }
        if self.text.focus_match(forward) {
            self.sync_to_match();
        } else {
            let mut message = String::from("aucune occurrence de ");
            message.push_str(&self.find.query);
            self.set_status(message);
        }
        self.needs_rebuild = true;
        self.needs_find_bar = true;
    }

    fn replace_and_step(&mut self) {
        if self.text.current_match().is_none() {
            self.step_match(true);
            return;
        }
        if self.text.replace_current(&self.find.replacement) {
            self.refresh_view();
            if self.text.focus_match(true) {
                self.sync_to_match();
            }
        }
        self.needs_find_bar = true;
    }

    fn replace_every(&mut self) {
        let count = self.text.replace_all(&self.find.replacement);
        self.refresh_view();
        self.needs_find_bar = true;
        if count == 0 {
            self.set_status(String::from("aucune occurrence"));
        } else {
            self.set_status(format!("{count} remplacements"));
        }
    }

    fn paste_into_find(&mut self) {
        let Some(clip) = clipboard_read() else {
            return;
        };
        let line = clip.lines().next().unwrap_or("");
        if line.is_empty() {
            return;
        }
        if self.find.replacing {
            self.find.replacement.push_str(line);
            self.needs_find_bar = true;
        } else {
            self.find.query.push_str(line);
            self.run_search();
        }
    }

    fn toggle_shadows(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let enabled = !renderer.shadows();
        renderer.set_shadows(enabled);
        let message = if enabled { "ombres actives" } else { "ombres coupees" };
        self.set_status(String::from(message));
    }

    fn toggle_bevel(&mut self) {
        let enabled = match self.renderer.as_ref() {
            Some(renderer) => !renderer.bevel(),
            None => return,
        };
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_bevel(enabled, &self.font);
        }
        self.needs_rebuild = true;
        let message = if enabled { "biseau actif" } else { "biseau coupe" };
        self.set_status(String::from(message));
    }

    fn toggle_indent_depth(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let enabled = !renderer.depth_by_indent();
        renderer.set_depth_by_indent(enabled);
        self.needs_rebuild = true;
        let message = if enabled {
            "relief par indentation actif"
        } else {
            "relief par indentation coupe"
        };
        self.set_status(String::from(message));
    }

    fn next_font(&mut self) {
        if !self.font.next_family() {
            self.set_status(String::from("une seule fonte disponible"));
            return;
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.reset_font(&self.font);
        }
        self.focus_version = 0;
        self.needs_rebuild = true;
        self.needs_popup = true;
        self.needs_find_bar = true;
        let mut message = String::from("fonte ");
        message.push_str(self.font.name());
        self.set_status(message);
    }

    fn export_path(&self, extension: &str) -> PathBuf {
        match self.text.path.as_ref() {
            Some(path) => path.with_extension(extension),
            None => {
                let mut name = String::from("text3d.");
                name.push_str(extension);
                PathBuf::from(name)
            }
        }
    }

    fn sync_highlighter(&mut self) {
        self.text.sync();
        let Some(highlighter) = self.highlighter.as_mut() else {
            return;
        };
        let lines = render::visible_lines(&self.text);
        let version = self.text.version();
        if self.shaded_version == version
            && self.shaded_start == lines.start
            && self.shaded_end == lines.end
        {
            return;
        }
        self.shaded_version = version;
        self.shaded_start = lines.start;
        self.shaded_end = lines.end;
        let window = self.text.byte_range(lines);
        highlighter.update(self.text.source(), self.text.line_starts(), window);
    }

    fn export_mesh(&mut self, binary: bool) {
        self.sync_highlighter();
        let highlighter = self.highlighter.as_ref();
        let mesh = match self.renderer.as_ref() {
            Some(renderer) => renderer.scene_mesh(&self.text, &self.font, highlighter),
            None => return,
        };
        let path = self.export_path(if binary { "glb" } else { "obj" });
        let written = if binary {
            export::write_glb(&path, &mesh)
        } else {
            export::write_obj(&path, &mesh)
        };
        self.set_status(written.unwrap_or_else(|err| err));
    }

    fn capture_png(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f32();
        let wave = self.wave;
        let grid = self.grid;
        let shot = match self.renderer.as_mut() {
            Some(renderer) => renderer.capture(&self.camera, elapsed, wave, grid),
            None => return,
        };
        let path = self.export_path("png");
        let written = match shot {
            Ok((width, height, rgba)) => export::write_png(&path, width, height, &rgba),
            Err(err) => Err(err),
        };
        self.set_status(written.unwrap_or_else(|err| err));
    }

    fn handle_find_key(&mut self, event: &KeyEvent) -> bool {
        let command = self.modifiers.super_key() || self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::F1)
            | Key::Named(NamedKey::F2)
            | Key::Named(NamedKey::F3)
            | Key::Named(NamedKey::F4)
            | Key::Named(NamedKey::F5)
            | Key::Named(NamedKey::F6)
            | Key::Named(NamedKey::F7) => false,
            Key::Named(NamedKey::Escape) => {
                self.close_find();
                true
            }
            Key::Named(NamedKey::Enter) => {
                if command {
                    self.replace_every();
                } else if self.find.replacing {
                    self.replace_and_step();
                } else {
                    self.step_match(!shift);
                }
                true
            }
            Key::Named(NamedKey::Tab) => {
                self.find.replacing = !self.find.replacing;
                self.needs_find_bar = true;
                true
            }
            Key::Named(NamedKey::Backspace) => {
                if self.find.replacing {
                    self.find.replacement.pop();
                    self.needs_find_bar = true;
                } else {
                    self.find.query.pop();
                    self.run_search();
                }
                true
            }
            _ => {
                if command {
                    match letter_of(&event.logical_key) {
                        Some('s') | Some('q') => return false,
                        Some('f') => {
                            self.find.replacing = false;
                            self.needs_find_bar = true;
                        }
                        Some('g') => self.step_match(!shift),
                        Some('v') => self.paste_into_find(),
                        _ => {}
                    }
                    return true;
                }
                let Some(written) = event.text.as_ref() else {
                    return true;
                };
                let mut changed = false;
                let field = if self.find.replacing {
                    &mut self.find.replacement
                } else {
                    &mut self.find.query
                };
                for ch in written.chars() {
                    if !ch.is_control() {
                        field.push(ch);
                        changed = true;
                    }
                }
                if !changed {
                    return true;
                }
                if self.find.replacing {
                    self.needs_find_bar = true;
                } else {
                    self.run_search();
                }
                true
            }
        }
    }

    fn handle_command_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Space) => self.update_completion(true),
            Key::Named(NamedKey::ArrowLeft) if alt => {
                self.text.move_word(false, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowRight) if alt => {
                self.text.move_word(true, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.text.move_home(shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.text.move_end(shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::Home) => {
                self.text.move_document(false, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::End) => {
                self.text.move_document(true, shift);
                self.refresh_view();
                self.close_popup();
            }
            _ => match letter_of(&event.logical_key) {
                Some('s') => {
                    self.text.save();
                    self.status_since = Some(Instant::now());
                }
                Some('q') => event_loop.exit(),
                Some('a') => self.select_everything(),
                Some('c') => self.copy(),
                Some('x') => self.cut(),
                Some('v') => self.paste(),
                Some('z') if shift => self.redo_edit(),
                Some('z') => self.undo_edit(),
                Some('f') => self.open_find(),
                Some('g') => self.step_match(!shift),
                Some('e') if shift => self.export_mesh(true),
                Some('e') => self.export_mesh(false),
                Some('p') => self.capture_png(),
                _ => {}
            },
        }
    }

    fn handle_alt_key(&mut self, event: &KeyEvent) {
        let shift = self.modifiers.shift_key();
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowLeft) if shift => {
                self.text.move_word(false, true);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowRight) if shift => {
                self.text.move_word(true, true);
                self.refresh_view();
            }
            Key::Named(NamedKey::Backspace) => {
                self.text.delete_word_left();
                self.refresh_view();
                self.update_completion(false);
            }
            Key::Named(NamedKey::ArrowLeft) => self.camera.orbit(-40.0, 0.0),
            Key::Named(NamedKey::ArrowRight) => self.camera.orbit(40.0, 0.0),
            Key::Named(NamedKey::ArrowUp) => self.camera.orbit(0.0, -40.0),
            Key::Named(NamedKey::ArrowDown) => self.camera.orbit(0.0, 40.0),
            Key::Named(NamedKey::PageUp) => self.camera.zoom(1.0),
            Key::Named(NamedKey::PageDown) => self.camera.zoom(-1.0),
            _ => {}
        }
    }

    fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.find.open && self.handle_find_key(event) {
            return;
        }
        if self.modifiers.super_key() || self.modifiers.control_key() {
            self.handle_command_key(event, event_loop);
            return;
        }
        if self.modifiers.alt_key() {
            self.handle_alt_key(event);
            return;
        }

        if self.completion.active {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.close_popup();
                    return;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.completion.move_selection(-1);
                    self.needs_popup = true;
                    return;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    self.completion.move_selection(1);
                    self.needs_popup = true;
                    return;
                }
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                    self.accept_completion();
                    return;
                }
                Key::Named(NamedKey::ArrowLeft)
                | Key::Named(NamedKey::ArrowRight)
                | Key::Named(NamedKey::Home)
                | Key::Named(NamedKey::End)
                | Key::Named(NamedKey::PageUp)
                | Key::Named(NamedKey::PageDown) => self.close_popup(),
                _ => {}
            }
        }

        let shift = self.modifiers.shift_key();
        match &event.logical_key {
            Key::Named(NamedKey::Backspace) => {
                self.text.backspace();
                self.refresh_view();
                self.update_completion(false);
            }
            Key::Named(NamedKey::Delete) => {
                self.text.delete();
                self.refresh_view();
                self.update_completion(false);
            }
            Key::Named(NamedKey::Enter) => {
                self.text.insert_newline();
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::Tab) => {
                if !self.text.word_prefix().is_empty() {
                    self.update_completion(true);
                }
                if !self.completion.active {
                    self.text.insert_str("    ");
                    self.refresh_view();
                }
            }
            Key::Named(NamedKey::Escape) => {
                if self.text.selection().is_some() {
                    self.text.clear_selection();
                    self.refresh_view();
                }
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.text.move_left(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.text.move_right(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.text.move_vertical(-1, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.text.move_vertical(1, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::PageUp) => {
                self.text.move_vertical(-20, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::PageDown) => {
                self.text.move_vertical(20, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::Home) => {
                self.text.move_home(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::End) => {
                self.text.move_end(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::F1) => {
                self.camera.reset_orientation();
                self.focus_locked = false;
            }
            Key::Named(NamedKey::F2) => {
                self.wave = !self.wave;
                let message = if self.wave { "ondulation active" } else { "ondulation figee" };
                self.set_status(String::from(message));
            }
            Key::Named(NamedKey::F3) => {
                self.grid = !self.grid;
                let message = if self.grid { "grille visible" } else { "grille masquee" };
                self.set_status(String::from(message));
            }
            Key::Named(NamedKey::F4) => self.toggle_shadows(),
            Key::Named(NamedKey::F5) => self.next_font(),
            Key::Named(NamedKey::F6) => self.toggle_bevel(),
            Key::Named(NamedKey::F7) => self.toggle_indent_depth(),
            _ => {
                if let Some(written) = &event.text {
                    let mut inserted = false;
                    for ch in written.chars() {
                        if !ch.is_control() {
                            self.text.insert_char(ch);
                            inserted = true;
                        }
                    }
                    if inserted {
                        self.refresh_view();
                        self.update_completion(false);
                    }
                }
            }
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let elapsed = (now - self.start).as_secs_f32();

        if self.renderer.is_none() {
            return;
        }

        if !self.text.status.is_empty() {
            match self.status_since {
                None => self.status_since = Some(now),
                Some(since) if (now - since).as_secs_f32() > STATUS_LIFETIME => {
                    self.text.status.clear();
                    self.status_since = None;
                }
                Some(_) => {}
            }
        }

        self.update_focus();
        self.camera.update(dt);

        let eye = self.camera.eye();
        if (eye - self.rebuild_eye).length() > self.rebuild_distance * LOD_TRAVEL_RATIO {
            self.needs_rebuild = true;
        }

        if self.needs_rebuild {
            self.needs_rebuild = false;
            self.needs_popup = true;
            self.needs_find_bar = true;
            self.sync_highlighter();
            let highlighter = self.highlighter.as_ref();
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.rebuild(&self.text, &self.font, highlighter, &self.camera);
            }
            self.rebuild_eye = eye;
            self.rebuild_distance = self.camera.distance.max(1.0);
        }

        if self.needs_popup {
            self.needs_popup = false;
            let completion = &self.completion;
            let text = &self.text;
            let font = &self.font;
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_popup(completion, text, font);
            }
        }

        if self.needs_find_bar {
            self.needs_find_bar = false;
            let bar = if self.find.open {
                Some(FindBarView {
                    query: &self.find.query,
                    replacement: &self.find.replacement,
                    replacing: self.find.replacing,
                    current: self.text.current_match(),
                    total: self.text.matches().len(),
                })
            } else {
                None
            };
            let text = &self.text;
            let font = &self.font;
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_find_bar(bar, text, font);
            }
        }

        self.refresh_title();

        let wave = self.wave;
        let grid = self.grid;
        let outcome = match self.renderer.as_mut() {
            Some(renderer) => renderer.render(&self.camera, elapsed, wave, grid),
            None => return,
        };
        if let Err(err) = outcome
            && self.text.status != err
        {
            self.set_status(err);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("text3d")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("fenetre: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.viewport = (size.width.max(1) as f32, size.height.max(1) as f32);
        match Renderer::new(window.clone(), &self.font) {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(err) => {
                eprintln!("gpu: {err}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.update_focus();
        self.camera.snap();
        self.refresh_title();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.viewport = (size.width.max(1) as f32, size.height.max(1) as f32);
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event, event_loop),
            WindowEvent::MouseInput { state, button, .. } => match button {
                MouseButton::Left => {
                    if state == ElementState::Pressed {
                        self.press_left();
                    } else {
                        self.release_left();
                    }
                }
                MouseButton::Right | MouseButton::Middle => {
                    self.pointer.panning = state == ElementState::Pressed;
                }
                _ => {}
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::CursorLeft { .. } => self.pointer.release_all(),
            WindowEvent::Focused(false) => {
                self.pointer.release_all();
                self.modifiers = ModifiersState::empty();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 40.0,
                };
                self.camera.zoom(amount);
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn letter_of(key: &Key) -> Option<char> {
    let Key::Character(written) = key else {
        return None;
    };
    let mut chars = written.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(first.to_ascii_lowercase())
}

fn write_compact(out: &mut String, value: u32) {
    if value < 1_000 {
        let _ = write!(out, "{value}");
    } else if value < 100_000 {
        let _ = write!(out, "{:.1}k", value as f32 / 1_000.0);
    } else if value < 1_000_000 {
        let _ = write!(out, "{}k", value / 1_000);
    } else {
        let _ = write!(out, "{:.1}M", value as f32 / 1_000_000.0);
    }
}

fn clipboard_read() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn clipboard_write(text: &str) {
    let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else {
        return;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

fn keep_within(value: f32, target: f32, reach: f32) -> f32 {
    let low = target - reach;
    let high = target + reach;
    value.clamp(low.min(high), high.max(low))
}

fn main() {
    let font = match Font::load() {
        Ok(font) => font,
        Err(err) => {
            eprintln!("fonte introuvable: {err}");
            std::process::exit(1);
        }
    };

    let text = match std::env::args_os().nth(1) {
        Some(arg) => TextBuffer::open(&PathBuf::from(arg)),
        None => TextBuffer::from_str(DEMO, None),
    };

    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("boucle d evenements: {err}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Err(err) = event_loop.run_app(&mut App::new(font, text)) {
        eprintln!("execution: {err}");
    }
}
