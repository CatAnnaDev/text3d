mod atlas;
mod camera;
mod commands;
mod complete;
mod export;
mod extrude;
mod font;
mod history;
mod hud;
mod ide;
mod json;
mod layout;
mod lsp;
mod overlay;
mod pick;
mod project;
mod render;
mod search;
mod syntax;
mod tasks;
mod text;
mod workspace;
#[cfg(test)]
mod tests;

use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use camera::Camera;
use commands::Chord;
use complete::{Completion, MIN_PREFIX};
use font::Font;
use hud::{Hud, HudModel, Region, Target};
use ide::{Ide, Notice};
use layout::LineLayout;
use lsp::position::utf16_to_char;
use lsp::protocol::{Severity, SignatureInfo, Symbol};
use overlay::{Overlay, Panel, Row, RowKind, VISIBLE_ROWS};
use project::Project;
use render::{DiagnosticSpan, FindBarView, GutterMark, Renderer};
use tasks::Task;
use text::{Cursor, TextBuffer};
use workspace::Workspace;

const EDGE_PADDING: f32 = 1.2;
const STATUS_LIFETIME: f32 = 4.0;
const DRAG_THRESHOLD: f32 = 4.0;
const CLICK_SLACK: f32 = 5.0;
const MULTI_CLICK_DELAY: f32 = 0.4;
const LOD_TRAVEL_RATIO: f32 = 0.08;
const MATCH_PAD_COLUMNS: f32 = 16.0;
const MATCH_PAD_LINES: f32 = 6.0;
const PANEL_LIMIT: usize = 400;
const SPAN_LIMIT: usize = 4096;
const SPAN_LINES: usize = 64;
const FINDER_REFRESH: Duration = Duration::from_millis(400);
const HOVER_DELAY: Duration = Duration::from_millis(500);
const DEMO: &str = "\
bloc-notes en trois dimensions
------------------------------

chaque lettre est un maillage extrude et
biseaute, tessele depuis les contours de la
fonte puis rendu en instancie sur le GPU,
avec ombres portees et niveaux de detail.

cmd + O ouvre un projet: l arbre des
fichiers, les onglets, les diagnostics, la
recherche et la completion du serveur de
langage flottent en HUD devant la scene.

  projet
  cmd + O             ouvrir un projet
  cmd + P             ouverture rapide
  cmd + shift + O     symboles du document
  cmd + T             symboles du projet
  cmd + shift + F     chercher dans le projet
  cmd + shift + P     palette de commandes
  cmd + shift + E     arbre de fichiers
  ctrl + apostrophe   panneau de sortie
  cmd + shift + M     problemes

  langage
  F12, cmd + clic     aller a la definition
  shift + F12         references
  F2                  renommer
  shift + option + F  formater

  taches
  cmd + B             verifier
  cmd + shift + B     compiler
  cmd + R             lancer
  cmd + shift + T     tester
  cmd + shift + K     clippy
  cmd + point         arreter la tache

  onglets
  cmd + W             fermer l onglet
  ctrl + tab          onglet suivant
  ctrl + shift + tab  onglet precedent
  cmd + 1 a cmd + 9   aller a l onglet n
  ctrl + option + gauche/droite  sauts

  edition
  shift + fleches     etendre la selection
  cmd + alt + fleches se deplacer par mot
  cmd + A             tout selectionner
  cmd + C / X / V     copier, couper, coller
  cmd + Z             annuler
  cmd + shift + Z     refaire
  alt + retour        effacer le mot a gauche
  cmd + S             enregistrer
  cmd + option + S    tout enregistrer
  tab / ctrl + espace ouvrir la completion
  cmd + gauche/droite debut ou fin de ligne
  cmd + haut/bas      haut ou bas du document

  recherche dans le fichier
  cmd + F             ouvrir la barre
  entree              occurrence suivante
  shift + entree      occurrence precedente
  tab                 champ de remplacement
  cmd + entree        tout remplacer
  cmd + G             suivante sans la barre
  echap               fermer

  vue
  option + 1 recadrer     option + 2 ondulation
  option + 3 grille       option + 4 ombres
  option + 5 fonte        option + 6 biseau
  option + 7 relief       option + 8 numeros

  souris
  clic gauche         poser le curseur
  double clic         selectionner le mot
  triple clic         selectionner la ligne
  shift + glisser     selectionner a la souris
  glisser gauche      tourner autour du texte
  glisser droit       translater
  molette             zoom sur l editeur,
                      defilement sur un panneau

  export
  option + E          maillage obj
  option + shift + E  maillage glb
  option + P          capture png

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

struct EditorFrame {
    bias_x: f32,
    bias_y: f32,
    half_width: f32,
    half_height: f32,
    floor_x: f32,
    ceiling_y: f32,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    font: Font,
    ide: Ide,
    overlay: Overlay,
    hud: Hud,
    camera: Camera,
    pointer: Pointer,
    modifiers: ModifiersState,
    completion: Completion,
    completion_partial: bool,
    find: FindBar,
    start: Instant,
    last_frame: Instant,
    needs_rebuild: bool,
    needs_popup: bool,
    needs_find_bar: bool,
    needs_diagnostics: bool,
    focus_layout: LineLayout,
    focus_line: usize,
    focus_version: u64,
    focus_locked: bool,
    pick_layout: LineLayout,
    status: String,
    status_since: Option<Instant>,
    scroll: (f32, f32),
    viewport: (f32, f32),
    title: String,
    title_scratch: String,
    wave: bool,
    grid: bool,
    rebuild_eye: Vec3,
    rebuild_distance: f32,
    notices: Vec<Notice>,
    spans: Vec<DiagnosticSpan>,
    gutter: Vec<GutterMark>,
    files_scratch: Vec<PathBuf>,
    symbols: Vec<Symbol>,
    hover: Option<String>,
    signature: Option<SignatureInfo>,
    hover_since: Option<Instant>,
    folder: Option<Receiver<Option<PathBuf>>>,
    searched: String,
    search_hits: usize,
    search_running: bool,
    indexed: usize,
    finder_at: Instant,
    close_pending: Option<usize>,
    targets_seen: usize,
    pending_command: Option<commands::Command>,
    diagnostic_line: Option<usize>,
}

impl App {
    fn new(font: Font, ide: Ide) -> App {
        let now = Instant::now();
        App {
            window: None,
            renderer: None,
            font,
            ide,
            overlay: Overlay::new(),
            hud: Hud::new(),
            camera: Camera::new(),
            pointer: Pointer::new(),
            modifiers: ModifiersState::empty(),
            completion: Completion::new(),
            completion_partial: false,
            find: FindBar::default(),
            start: now,
            last_frame: now,
            needs_rebuild: true,
            needs_popup: true,
            needs_find_bar: true,
            needs_diagnostics: true,
            focus_layout: LineLayout::default(),
            focus_line: 0,
            focus_version: 0,
            focus_locked: false,
            pick_layout: LineLayout::default(),
            status: String::new(),
            status_since: None,
            scroll: (0.0, 0.0),
            viewport: (1280.0, 800.0),
            title: String::new(),
            title_scratch: String::new(),
            wave: true,
            grid: true,
            rebuild_eye: Vec3::ZERO,
            rebuild_distance: 1.0,
            notices: Vec::new(),
            spans: Vec::new(),
            gutter: Vec::new(),
            files_scratch: Vec::new(),
            symbols: Vec::new(),
            hover: None,
            signature: None,
            hover_since: None,
            folder: None,
            searched: String::new(),
            search_hits: 0,
            search_running: false,
            indexed: 0,
            finder_at: now,
            close_pending: None,
            targets_seen: 0,
            pending_command: None,
            diagnostic_line: None,
        }
    }

    fn set_status(&mut self, message: String) {
        self.status = message;
        self.status_since = Some(Instant::now());
    }

    fn status_message(&mut self, message: &str) {
        self.status.clear();
        self.status.push_str(message);
        self.status_since = Some(Instant::now());
    }

    fn refresh_view(&mut self) {
        self.needs_rebuild = true;
        self.focus_locked = false;
        self.hover = None;
        self.hover_since = Some(Instant::now());
    }

    fn after_edit(&mut self) {
        self.needs_rebuild = true;
        self.needs_diagnostics = true;
        self.focus_locked = false;
        self.hover = None;
        self.hover_since = None;
        self.ide.notify_edit();
    }

    fn after_activate(&mut self) {
        self.ide.notify_activated();
        self.scroll = self.ide.workspace().scroll();
        self.focus_version = 0;
        self.focus_locked = false;
        self.needs_rebuild = true;
        self.needs_diagnostics = true;
        self.needs_popup = true;
        self.needs_find_bar = true;
        self.hover = None;
        self.signature = None;
        self.hover_since = None;
        self.close_pending = None;
        self.completion.dismiss();
        if let Some(path) = self.ide.workspace().buffer().path.clone() {
            self.ide.project_mut().expand_to(&path);
        }
    }

    fn update_completion(&mut self, force: bool) {
        self.needs_popup = true;
        let App { ide, completion, .. } = self;
        let workspace = ide.workspace_mut();
        workspace.buffer_mut().sync();
        let language = workspace.language();
        if !force && language.is_none() && !completion.active {
            completion.dismiss();
            return;
        }
        let min_prefix = if force { 1 } else { MIN_PREFIX };
        let workspace = ide.workspace();
        completion.refresh(workspace.buffer(), language, workspace.highlighter(), min_prefix);
    }

    fn ask_language_completion(&mut self, previous: Option<char>, typed: char) {
        if typed == '(' || typed == ',' {
            self.ide.ask_signature();
        }
        if !self.ide.server_ready() {
            return;
        }
        if let Some(trigger) = commands::completion_trigger(previous, typed) {
            self.ide.ask_completion(Some(trigger));
            return;
        }
        if !text::is_word_char(typed) {
            return;
        }
        let prefix = self.ide.workspace().buffer().word_prefix().chars().count();
        if prefix >= MIN_PREFIX || self.completion_partial {
            self.ide.ask_completion(None);
        }
    }

    fn accept_completion(&mut self) {
        let Some(candidate) = self.completion.selection() else {
            return;
        };
        let replacement = candidate.text.clone();
        let removed = self.completion.prefix_chars;
        let buffer = self.ide.workspace_mut().buffer_mut();
        for _ in 0..removed {
            buffer.backspace();
        }
        buffer.insert_str(&replacement);
        self.completion.dismiss();
        self.after_edit();
        self.needs_popup = true;
    }

    fn close_popup(&mut self) {
        self.completion.dismiss();
        self.completion_partial = false;
        self.needs_popup = true;
    }

    fn aspect(&self) -> f32 {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.aspect())
            .unwrap_or(1.6)
    }

    fn cursor_x(&mut self) -> f32 {
        let App { ide, font, focus_layout, focus_line, focus_version, .. } = self;
        let buffer = ide.workspace().buffer();
        let version = buffer.version();
        if *focus_version != version || *focus_line != buffer.cursor_line {
            *focus_version = version;
            *focus_line = buffer.cursor_line;
            focus_layout.build(font, buffer.lines[buffer.cursor_line].as_str());
        }
        focus_layout.x_of_column(buffer.cursor_col)
    }

    fn editor_frame(&self) -> EditorFrame {
        let (half_width, half_height) = self.camera.half_extent(self.aspect());
        let plain = EditorFrame {
            bias_x: 0.0,
            bias_y: 0.0,
            half_width,
            half_height,
            floor_x: half_width * 0.90 - EDGE_PADDING,
            ceiling_y: EDGE_PADDING - half_height * 0.90,
        };
        let Some(renderer) = self.renderer.as_ref() else {
            return plain;
        };
        let view = renderer.hud_viewport(&self.camera);
        let rect = self.hud.editor_rect();
        if !(view.width > 0.0 && view.height > 0.0 && rect.w > 0.0 && rect.h > 0.0) {
            return plain;
        }
        let scale_x = half_width * 2.0 / view.width;
        let scale_y = half_height * 2.0 / view.height;
        let left = (view.width * 0.5 - rect.x) * scale_x;
        let top = (view.height * 0.5 - rect.y) * scale_y;
        EditorFrame {
            bias_x: (rect.x + rect.w * 0.5 - view.width * 0.5) * scale_x,
            bias_y: (view.height * 0.5 - rect.y - rect.h * 0.5) * scale_y,
            half_width: rect.w * 0.5 * scale_x,
            half_height: rect.h * 0.5 * scale_y,
            floor_x: left * 0.90 - EDGE_PADDING,
            ceiling_y: EDGE_PADDING - top * 0.90,
        }
    }

    fn update_focus(&mut self) {
        if self.focus_locked {
            self.camera
                .set_focus(Vec3::new(self.scroll.0, self.scroll.1, 0.0));
            return;
        }
        let frame = self.editor_frame();
        let cursor_x = self.cursor_x();
        let line_height = self.font.line_height();
        let cursor_line = self.ide.workspace().buffer().cursor_line;
        let cursor_y = -(cursor_line as f32) * line_height;

        let goal_x = cursor_x - frame.bias_x;
        let reach_x = (frame.half_width * 0.85).max(0.5);
        self.scroll.0 = keep_within(self.scroll.0, goal_x, reach_x).max(frame.floor_x);

        let goal_y = cursor_y - frame.bias_y;
        let reach_y = (frame.half_height * 0.82).max(0.5);
        self.scroll.1 = keep_within(self.scroll.1, goal_y, reach_y);
        if self.completion.active {
            let rows = self.completion.items.len().min(complete::VISIBLE_ROWS) as f32;
            let popup_bottom = goal_y - (rows + 0.6) * line_height;
            self.scroll.1 = keep_within(self.scroll.1, popup_bottom, reach_y);
        }
        self.scroll.1 = self.scroll.1.min(frame.ceiling_y);

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
        let (errors, warnings) = self.ide.diagnostic_counts();
        self.title_scratch.clear();
        self.title_scratch.push_str("text3d  |  ");
        self.title_scratch.push_str(self.ide.project().label());
        self.title_scratch.push_str("  |  ");
        let buffer = self.ide.workspace().buffer();
        match buffer.path.as_deref().and_then(|path| path.file_name()) {
            Some(name) => self.title_scratch.push_str(&name.to_string_lossy()),
            None => self.title_scratch.push_str("sans titre"),
        }
        if buffer.modified {
            self.title_scratch.push_str(" *");
        }
        let _ = write!(
            self.title_scratch,
            "  |  {}:{}",
            buffer.cursor_line + 1,
            buffer.cursor_col + 1
        );
        if let Some(language) = self.ide.workspace().language() {
            self.title_scratch.push_str("  |  ");
            self.title_scratch.push_str(language.label());
        }
        self.title_scratch.push_str("  |  ");
        self.title_scratch.push_str(self.font.name());
        if errors > 0 || warnings > 0 {
            let _ = write!(self.title_scratch, "  |  {errors} err {warnings} avt");
        }
        if let Some(stats) = stats {
            self.title_scratch.push_str("  |  ");
            write_compact(&mut self.title_scratch, stats.instances);
            self.title_scratch.push_str(" glyphes  |  ");
            write_compact(&mut self.title_scratch, stats.triangles);
            self.title_scratch.push_str(" tri");
        }
        if !self.status.is_empty() {
            self.title_scratch.push_str("  |  ");
            self.title_scratch.push_str(&self.status);
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
        let App { renderer, camera, viewport, ide, font, pick_layout, .. } = self;
        let aspect = renderer.as_ref()?.aspect();
        pick::hit_text(
            camera,
            aspect,
            viewport.0,
            viewport.1,
            x,
            y,
            ide.workspace().buffer(),
            font,
            pick_layout,
        )
    }

    fn hud_point(&self, x: f32, y: f32) -> (f32, f32) {
        let Some(renderer) = self.renderer.as_ref() else {
            return (0.0, 0.0);
        };
        let view = renderer.hud_viewport(&self.camera);
        (
            x / self.viewport.0.max(1.0) * view.width,
            y / self.viewport.1.max(1.0) * view.height,
        )
    }

    fn press_left(&mut self) {
        let at = self.pointer.last;
        let (hud_x, hud_y) = self.hud_point(at.0, at.1);
        let target = self.hud.hit(hud_x, hud_y);
        if target == Target::Editor && self.overlay.is_capturing_input() {
            self.pointer.pressed = false;
            self.pointer.handled = true;
            self.close_panel();
            return;
        }
        if target != Target::Editor {
            self.pointer.pressed = false;
            self.pointer.handled = true;
            self.pointer.clicks = 0;
            self.pointer.click_time = None;
            self.activate_target(target);
            return;
        }

        let now = Instant::now();
        let near =
            (at.0 - self.pointer.click_at.0).hypot(at.1 - self.pointer.click_at.1) <= CLICK_SLACK;
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

        if self.modifiers.super_key() {
            self.pointer.pressed = false;
            self.pointer.handled = true;
            if let Some(hit) = self.pick_at(at.0, at.1) {
                self.ide
                    .workspace_mut()
                    .buffer_mut()
                    .set_cursor(Cursor { line: hit.line, column: hit.column }, false);
                self.refresh_view();
                self.close_popup();
                self.request_definition();
            }
            return;
        }

        let clicks = self.pointer.clicks;
        if clicks == 1 && !self.pointer.selecting {
            return;
        }
        let Some(hit) = self.pick_at(at.0, at.1) else {
            return;
        };
        let cursor = Cursor { line: hit.line, column: hit.column };
        let buffer = self.ide.workspace_mut().buffer_mut();
        match clicks {
            2 => buffer.select_word_at(cursor),
            3 => buffer.select_line_at(hit.line),
            _ => buffer.set_cursor(cursor, false),
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
        self.ide
            .workspace_mut()
            .buffer_mut()
            .set_cursor(Cursor { line: hit.line, column: hit.column }, false);
        self.refresh_view();
        self.close_popup();
    }

    fn cursor_moved(&mut self, x: f32, y: f32) {
        let delta = (x - self.pointer.last.0, y - self.pointer.last.1);
        self.pointer.last = (x, y);

        if self.pointer.selecting {
            if let Some(hit) = self.pick_at(x, y) {
                self.ide
                    .workspace_mut()
                    .buffer_mut()
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

    fn wheel(&mut self, amount: f32) {
        if amount == 0.0 {
            return;
        }
        let at = self.pointer.last;
        let (hud_x, hud_y) = self.hud_point(at.0, at.1);
        let magnitude = amount.abs().max(1.0).round() as isize;
        let steps = if amount > 0.0 { -magnitude } else { magnitude };
        match self.hud.region_at(hud_x, hud_y) {
            Some(Region::Tree) => {
                let total = self.ide.project().entries().len();
                self.overlay.scroll_tree(steps, total);
            }
            Some(Region::Output) => {
                let total = self.ide.tasks().lines().len();
                self.overlay.scroll_output(-steps, total);
            }
            Some(Region::Panel) => self.overlay.move_selection(steps),
            None => {
                if self.hud.editor_rect().contains(hud_x, hud_y) {
                    self.camera.zoom(amount);
                }
            }
        }
    }

    fn activate_target(&mut self, target: Target) {
        match target {
            Target::None | Target::Editor => {}
            Target::TreeRow(index) => self.open_tree_row(index, false),
            Target::TreeToggle(index) => self.open_tree_row(index, true),
            Target::Tab(index) => self.activate_tab(index),
            Target::TabClose(index) => self.close_tab(index),
            Target::PanelRow(index) => {
                let picked = self.overlay.select_at(index).map(describe_row);
                if let Some(picked) = picked {
                    self.take_row(picked);
                }
            }
            Target::OutputLine(index) => self.open_output_line(index),
            Target::StatusDiagnostics => self.open_problems(),
            Target::StatusServer => {
                let mut message = String::from("serveur: ");
                message.push_str(self.ide.server_status());
                self.set_status(message);
            }
            Target::Scrollbar(region) => {
                let steps = VISIBLE_ROWS as isize;
                match region {
                    Region::Tree => {
                        let total = self.ide.project().entries().len();
                        self.overlay.scroll_tree(steps, total);
                    }
                    Region::Output => {
                        let total = self.ide.tasks().lines().len();
                        self.overlay.scroll_output(steps, total);
                    }
                    Region::Panel => self.overlay.move_selection(steps),
                }
            }
        }
    }

    fn open_tree_row(&mut self, index: usize, toggle_only: bool) {
        let Some(entry) = self.ide.project().entries().get(index) else {
            return;
        };
        let directory = entry.is_dir;
        let path = if directory { PathBuf::new() } else { entry.path.clone() };
        self.overlay.set_sidebar_selection(index);
        let total = self.ide.project().entries().len();
        self.overlay.reveal_tree(index, total);
        if directory || toggle_only {
            self.ide.project_mut().toggle(index);
            return;
        }
        self.open_path(&path, None);
    }

    fn open_output_line(&mut self, index: usize) {
        let Some(line) = self.ide.tasks().lines().get(index) else {
            return;
        };
        let Some((path, row, column)) = commands::location_in_line(&line.text) else {
            return;
        };
        let full = if path.is_absolute() {
            path
        } else {
            self.ide.project().root().join(path)
        };
        let cursor = Cursor { line: row.saturating_sub(1), column: column.saturating_sub(1) };
        self.jump_to(&full, cursor, false);
    }

    fn open_path(&mut self, path: &Path, cursor: Option<Cursor>) {
        self.ide.workspace_mut().set_scroll(self.scroll);
        match self.ide.open_file(path, cursor) {
            Ok(()) => self.after_activate(),
            Err(err) => self.set_status(err),
        }
    }

    fn jump_to(&mut self, path: &Path, cursor: Cursor, utf16: bool) {
        self.ide.workspace_mut().push_jump();
        let seed = Cursor {
            line: cursor.line,
            column: if utf16 { 0 } else { cursor.column },
        };
        self.open_path(path, Some(seed));
        if !utf16 {
            return;
        }
        let column = {
            let buffer = self.ide.workspace().buffer();
            let line = buffer
                .lines
                .get(cursor.line)
                .map(String::as_str)
                .unwrap_or("");
            utf16_to_char(line, cursor.column as u32)
        };
        self.ide
            .workspace_mut()
            .buffer_mut()
            .set_cursor(Cursor { line: cursor.line, column }, false);
        self.refresh_view();
    }

    fn select_everything(&mut self) {
        let advance = self.font.advance();
        let line_height = self.font.line_height();
        let buffer = self.ide.workspace_mut().buffer_mut();
        buffer.select_all();
        let mut widest = 0;
        for line in &buffer.lines {
            let count = line.chars().count();
            if count > widest {
                widest = count;
            }
        }
        let last = (buffer.line_count().max(1) - 1) as f32;
        let min = Vec3::new(-advance, -last * line_height - line_height, 0.0);
        let max = Vec3::new(widest as f32 * advance + advance, line_height, 0.0);
        self.refresh_view();
        self.close_popup();
        self.frame_on(min, max);
        self.status_message("tout selectionne");
    }

    fn copy(&mut self) {
        let buffer = self.ide.workspace().buffer();
        let message = match buffer.selected_text() {
            Some(selected) => {
                clipboard_write(&selected);
                "selection copiee"
            }
            None => {
                clipboard_write(buffer.current_line());
                "ligne copiee"
            }
        };
        self.status_message(message);
    }

    fn cut(&mut self) {
        let buffer = self.ide.workspace_mut().buffer_mut();
        let selected = buffer.selected_text();
        let message = match selected {
            Some(selected) => {
                clipboard_write(&selected);
                buffer.delete_selection();
                "selection coupee"
            }
            None => {
                clipboard_write(buffer.current_line());
                buffer.delete_line();
                "ligne coupee"
            }
        };
        self.after_edit();
        self.close_popup();
        self.status_message(message);
    }

    fn paste(&mut self) {
        let Some(clip) = clipboard_read() else {
            self.status_message("presse-papier vide");
            return;
        };
        self.ide.workspace_mut().buffer_mut().insert_text(&clip);
        self.after_edit();
        self.close_popup();
    }

    fn undo_edit(&mut self) {
        if self.ide.workspace_mut().buffer_mut().undo() {
            self.after_edit();
            self.close_popup();
        } else {
            self.status_message("rien a annuler");
        }
    }

    fn redo_edit(&mut self) {
        if self.ide.workspace_mut().buffer_mut().redo() {
            self.after_edit();
            self.close_popup();
        } else {
            self.status_message("rien a refaire");
        }
    }

    fn open_find(&mut self) {
        self.find.open = true;
        self.find.replacing = false;
        self.close_popup();
        if let Some(selected) = self.ide.workspace().buffer().selected_text()
            && !selected.contains('\n')
        {
            self.find.query.clear();
            self.find.query.push_str(&selected);
        }
        if self.find.query.is_empty() {
            self.needs_find_bar = true;
            self.needs_rebuild = true;
        } else {
            self.run_find();
        }
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.replacing = false;
        self.ide.workspace_mut().buffer_mut().clear_matches();
        self.needs_find_bar = true;
        self.refresh_view();
    }

    fn run_find(&mut self) {
        let buffer = self.ide.workspace_mut().buffer_mut();
        buffer.find_all(&self.find.query, false);
        let focused = buffer.focus_match(true);
        if focused {
            self.sync_to_match();
        }
        self.needs_rebuild = true;
        self.needs_find_bar = true;
    }

    fn sync_to_match(&mut self) {
        let buffer = self.ide.workspace().buffer();
        let Some(index) = buffer.current_match() else {
            return;
        };
        let Some(found) = buffer.matches().get(index).copied() else {
            return;
        };
        let buffer = self.ide.workspace_mut().buffer_mut();
        buffer.set_cursor(Cursor { line: found.line, column: found.end }, false);
        buffer.set_cursor(Cursor { line: found.line, column: found.start }, true);
        self.needs_rebuild = true;
        self.focus_locked = false;

        let line_height = self.font.line_height();
        let y = -(found.line as f32) * line_height;
        {
            let App { ide, font, focus_layout, focus_line, focus_version, .. } = self;
            let buffer = ide.workspace().buffer();
            *focus_version = buffer.version();
            *focus_line = found.line;
            focus_layout.build(font, buffer.lines[found.line].as_str());
        }
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
            self.status_message("aucune recherche");
            return;
        }
        let buffer = self.ide.workspace_mut().buffer_mut();
        if buffer.matches().is_empty() {
            buffer.find_all(&self.find.query, false);
        }
        let focused = buffer.focus_match(forward);
        if focused {
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
        if self.ide.workspace().buffer().current_match().is_none() {
            self.step_match(true);
            return;
        }
        let buffer = self.ide.workspace_mut().buffer_mut();
        let replaced = buffer.replace_current(&self.find.replacement);
        let focused = replaced && buffer.focus_match(true);
        if replaced {
            self.after_edit();
        }
        if focused {
            self.sync_to_match();
        }
        self.needs_find_bar = true;
    }

    fn replace_every(&mut self) {
        let count = self
            .ide
            .workspace_mut()
            .buffer_mut()
            .replace_all(&self.find.replacement);
        self.after_edit();
        self.needs_find_bar = true;
        if count == 0 {
            self.status_message("aucune occurrence");
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
            self.run_find();
        }
    }

    fn toggle_shadows(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let enabled = !renderer.shadows();
        renderer.set_shadows(enabled);
        self.status_message(if enabled { "ombres actives" } else { "ombres coupees" });
    }

    fn toggle_wave(&mut self) {
        self.wave = !self.wave;
        self.status_message(if self.wave { "ondulation active" } else { "ondulation figee" });
    }

    fn toggle_grid(&mut self) {
        self.grid = !self.grid;
        self.status_message(if self.grid { "grille visible" } else { "grille masquee" });
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
        self.status_message(if enabled { "biseau actif" } else { "biseau coupe" });
    }

    fn toggle_indent_depth(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let enabled = !renderer.depth_by_indent();
        renderer.set_depth_by_indent(enabled);
        self.needs_rebuild = true;
        self.status_message(if enabled {
            "relief par indentation actif"
        } else {
            "relief par indentation coupe"
        });
    }

    fn toggle_line_numbers(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let enabled = !renderer.line_numbers();
        renderer.set_line_numbers(enabled);
        self.needs_rebuild = true;
        self.status_message(if enabled {
            "numeros de ligne actifs"
        } else {
            "numeros de ligne coupes"
        });
    }

    fn toggle_sidebar(&mut self) {
        self.overlay.toggle_sidebar();
        self.status_message(if self.overlay.sidebar() {
            "arbre de fichiers visible"
        } else {
            "arbre de fichiers masque"
        });
    }

    fn toggle_output(&mut self) {
        self.overlay.toggle_output();
        self.status_message(if self.overlay.output() {
            "panneau de sortie visible"
        } else {
            "panneau de sortie masque"
        });
    }

    fn recenter(&mut self) {
        self.camera.reset_orientation();
        self.focus_locked = false;
        self.status_message("vue recadree");
    }

    fn next_font(&mut self) {
        if !self.font.next_family() {
            self.status_message("une seule fonte disponible");
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
        match self.ide.workspace().buffer().path.as_ref() {
            Some(path) => path.with_extension(extension),
            None => {
                let mut name = String::from("text3d.");
                name.push_str(extension);
                PathBuf::from(name)
            }
        }
    }

    fn sync_shading(&mut self) {
        let lines = render::visible_lines(self.ide.workspace().buffer());
        self.ide.workspace_mut().sync_highlighter(lines);
    }

    fn export_mesh(&mut self, binary: bool) {
        self.sync_shading();
        let path = self.export_path(if binary { "glb" } else { "obj" });
        let mesh = {
            let App { renderer, ide, font, .. } = self;
            let Some(renderer) = renderer.as_ref() else {
                return;
            };
            let workspace = ide.workspace();
            renderer.scene_mesh(workspace.buffer(), font, workspace.highlighter())
        };
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

    fn open_panel(&mut self, panel: Panel) {
        self.close_popup();
        if self.find.open {
            self.close_find();
        }
        self.overlay.open(panel);
    }

    fn close_panel(&mut self) {
        self.overlay.close();
        self.symbols.clear();
    }

    fn showing(&self, panel: Panel) -> bool {
        self.overlay.panel() == panel
    }

    fn open_quick_open(&mut self) {
        self.open_panel(Panel::QuickOpen);
        self.sync_finder(true);
        self.refresh_panel_rows();
    }

    fn open_document_symbols(&mut self) {
        self.open_panel(Panel::DocumentSymbols);
        self.symbols.clear();
        self.announce_server();
        self.ide.ask_document_symbols();
    }

    fn open_workspace_symbols(&mut self) {
        self.open_panel(Panel::WorkspaceSymbols);
        self.symbols.clear();
        self.announce_server();
        self.ide.ask_workspace_symbols("");
    }

    fn announce_server(&mut self) {
        if self.ide.server_ready() {
            self.overlay
                .set_status(String::from("interrogation du serveur"));
            return;
        }
        let mut message = String::from("serveur indisponible: ");
        message.push_str(self.ide.server_status());
        self.overlay.set_status(message);
    }

    fn open_project_search(&mut self) {
        let selected = self
            .ide
            .workspace()
            .buffer()
            .selected_text()
            .filter(|value| !value.contains('\n'));
        self.open_panel(Panel::Search);
        if let Some(selected) = selected {
            self.overlay.set_query(selected);
        }
        self.searched.clear();
        self.overlay
            .set_status(String::from("entree pour lancer la recherche"));
    }

    fn open_problems(&mut self) {
        self.open_panel(Panel::Problems);
        self.refresh_panel_rows();
    }

    fn open_rename(&mut self) {
        let word = self.word_at_cursor();
        if word.is_empty() {
            self.status_message("aucun symbole sous le curseur");
            return;
        }
        self.open_panel(Panel::Rename);
        self.overlay.open_with(Panel::Rename, word);
        self.announce_server();
    }

    fn open_commands(&mut self) {
        self.open_panel(Panel::Commands);
        self.refresh_panel_rows();
    }

    fn request_definition(&mut self) {
        if !self.ide.server_ready() {
            self.status_message("aucun serveur de langage");
            return;
        }
        self.ide.ask_definition();
        self.status_message("recherche de la definition");
    }

    fn request_references(&mut self) {
        if !self.ide.server_ready() {
            self.status_message("aucun serveur de langage");
            return;
        }
        self.ide.ask_references();
        self.open_panel(Panel::References);
        self.symbols.clear();
        self.announce_server();
    }

    fn request_format(&mut self) {
        if !self.ide.server_ready() {
            self.status_message("aucun serveur de langage");
            return;
        }
        self.ide.ask_format();
    }

    fn word_at_cursor(&self) -> String {
        let buffer = self.ide.workspace().buffer();
        let Some(line) = buffer.lines.get(buffer.cursor_line) else {
            return String::new();
        };
        let at = line
            .char_indices()
            .nth(buffer.cursor_col)
            .map(|(byte, _)| byte)
            .unwrap_or(line.len());
        let start = line[..at]
            .char_indices()
            .rev()
            .take_while(|(_, ch)| text::is_word_char(*ch))
            .last()
            .map(|(byte, _)| byte)
            .unwrap_or(at);
        let mut end = at;
        for ch in line[at..].chars() {
            if !text::is_word_char(ch) {
                break;
            }
            end += ch.len_utf8();
        }
        line[start..end].to_string()
    }

    fn sync_finder(&mut self, force: bool) {
        let (count, scanning) = self.ide.project().indexed();
        if !force {
            if count == self.indexed {
                return;
            }
            if scanning && self.finder_at.elapsed() < FINDER_REFRESH {
                return;
            }
        }
        self.indexed = count;
        self.finder_at = Instant::now();
        self.files_scratch.clear();
        self.files_scratch
            .extend_from_slice(self.ide.project().files());
        let root = self.ide.project().root().to_path_buf();
        self.ide.finder_mut().set_files(&self.files_scratch, &root);
    }

    fn refresh_panel_rows(&mut self) {
        match self.overlay.panel() {
            Panel::QuickOpen => self.rows_files(),
            Panel::DocumentSymbols | Panel::WorkspaceSymbols | Panel::References => {
                self.rows_symbols()
            }
            Panel::Search => self.rows_hits(),
            Panel::Problems => self.rows_problems(),
            Panel::Commands => self.rows_commands(),
            Panel::Rename | Panel::None => return,
        }
        if self.overlay.rows().is_empty() {
            let mut rows = self.overlay.take_rows();
            rows.clear();
            rows.push(Row::plain(String::from("aucun resultat")));
            self.overlay.set_rows(rows);
        }
    }

    fn rows_files(&mut self) {
        self.sync_finder(false);
        let App { ide, overlay, .. } = self;
        let mut rows = overlay.take_rows();
        rows.clear();
        let found = ide.finder_mut().query(overlay.query(), PANEL_LIMIT);
        rows.reserve(found.len());
        for item in found {
            let (folder, name) = match item.display.rsplit_once('/') {
                Some((folder, name)) => (folder, name),
                None => ("", item.display.as_str()),
            };
            rows.push(Row::new(
                String::from(name),
                String::from(folder),
                "fic",
                RowKind::File,
                Some((item.path.clone(), Cursor { line: 0, column: 0 })),
            ));
        }
        let shown = rows.len();
        overlay.set_rows(rows);
        let total = ide.project().files().len();
        let status = overlay.status_mut();
        status.clear();
        let _ = write!(status, "{shown} sur {total} fichiers");
    }

    fn rows_symbols(&mut self) {
        let reference = self.overlay.panel() == Panel::References;
        let App { overlay, symbols, .. } = self;
        let needle = overlay.query().to_ascii_lowercase();
        let mut rows = overlay.take_rows();
        rows.clear();
        rows.reserve(symbols.len().min(PANEL_LIMIT));
        for symbol in symbols.iter() {
            if rows.len() >= PANEL_LIMIT {
                break;
            }
            if !needle.is_empty() && !symbol.name.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let mut detail = String::with_capacity(symbol.container.len() + 24);
            if !symbol.container.is_empty() {
                detail.push_str(&symbol.container);
                detail.push_str("  ");
            }
            if let Some(name) = symbol.path.file_name() {
                detail.push_str(&name.to_string_lossy());
            }
            let _ = write!(detail, ":{}", symbol.selection.start.line + 1);
            let mut label = String::with_capacity(symbol.name.len() + symbol.depth * 2);
            for _ in 0..symbol.depth.min(8) {
                label.push_str("  ");
            }
            label.push_str(&symbol.name);
            rows.push(Row::new(
                label,
                detail,
                symbol_tag(symbol.kind),
                if reference { RowKind::Reference } else { RowKind::Symbol },
                Some((
                    symbol.path.clone(),
                    Cursor {
                        line: symbol.selection.start.line as usize,
                        column: symbol.selection.start.character as usize,
                    },
                )),
            ));
        }
        let shown = rows.len();
        let total = symbols.len();
        overlay.set_rows(rows);
        let status = overlay.status_mut();
        status.clear();
        let _ = write!(status, "{shown} sur {total} symboles");
    }

    fn rows_hits(&mut self) {
        let App { ide, overlay, .. } = self;
        let mut rows = overlay.take_rows();
        rows.clear();
        let search = ide.search();
        let hits = search.hits();
        rows.reserve(hits.len().min(PANEL_LIMIT));
        for hit in hits.iter().take(PANEL_LIMIT) {
            let mut detail = String::with_capacity(32);
            if let Some(name) = hit.path.file_name() {
                detail.push_str(&name.to_string_lossy());
            }
            let _ = write!(detail, ":{}", hit.line + 1);
            rows.push(Row::new(
                String::from(hit.preview.trim_end()),
                detail,
                "occ",
                RowKind::Match,
                Some((hit.path.clone(), Cursor { line: hit.line, column: hit.column })),
            ));
        }
        let total = hits.len();
        let (done, files) = search.progress();
        let running = search.running();
        let capped = search.capped();
        overlay.set_rows(rows);
        let status = overlay.status_mut();
        status.clear();
        let _ = write!(status, "{total} occurrences");
        if !search.needle().is_empty() {
            let _ = write!(status, " de {}", search.needle());
        }
        if running {
            let _ = write!(status, ", {done} sur {files} fichiers");
        } else if capped {
            status.push_str(", plafond atteint");
        }
    }

    fn rows_problems(&mut self) {
        let App { ide, overlay, .. } = self;
        let needle = overlay.query().to_ascii_lowercase();
        let mut rows = overlay.take_rows();
        rows.clear();
        let all = ide.all_diagnostics();
        rows.reserve(all.len().min(PANEL_LIMIT));
        for (path, diagnostic) in all.iter() {
            if rows.len() >= PANEL_LIMIT {
                break;
            }
            if !needle.is_empty() && !diagnostic.message.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let message = diagnostic.message.lines().next().unwrap_or("");
            let mut detail = String::with_capacity(32);
            if let Some(name) = path.file_name() {
                detail.push_str(&name.to_string_lossy());
            }
            let _ = write!(detail, ":{}", diagnostic.range.start.line + 1);
            if !diagnostic.code.is_empty() {
                detail.push_str("  ");
                detail.push_str(&diagnostic.code);
            }
            rows.push(Row::new(
                String::from(message),
                detail,
                severity_tag(diagnostic.severity),
                severity_kind(diagnostic.severity),
                Some((
                    path.clone(),
                    Cursor {
                        line: diagnostic.range.start.line as usize,
                        column: diagnostic.range.start.character as usize,
                    },
                )),
            ));
        }
        let shown = rows.len();
        let total = all.len();
        overlay.set_rows(rows);
        let status = overlay.status_mut();
        status.clear();
        let _ = write!(status, "{shown} sur {total} problemes");
    }

    fn rows_commands(&mut self) {
        let needle = self.overlay.query().to_ascii_lowercase();
        let table = commands::all();
        let mut rows = self.overlay.take_rows();
        rows.clear();
        rows.reserve(table.len());
        for command in table {
            let label = commands::label(*command);
            if !needle.is_empty() && !label.contains(&needle) {
                continue;
            }
            rows.push(Row::new(
                String::from(label),
                String::from(commands::shortcut(*command)),
                "cmd",
                RowKind::Command,
                None,
            ));
        }
        let shown = rows.len();
        self.overlay.set_rows(rows);
        let status = self.overlay.status_mut();
        status.clear();
        let _ = write!(status, "{shown} sur {} commandes", table.len());
    }

    fn on_query_changed(&mut self) {
        if self.overlay.panel() == Panel::WorkspaceSymbols {
            self.ide.ask_workspace_symbols(self.overlay.query());
        }
        self.refresh_panel_rows();
    }

    fn accept_panel(&mut self) {
        match self.overlay.panel() {
            Panel::Rename => {
                let name = self.overlay.query().to_string();
                self.close_panel();
                if name.is_empty() {
                    self.status_message("nom vide");
                    return;
                }
                self.ide.ask_rename(&name);
                self.status_message("renommage demande");
                return;
            }
            Panel::Search => {
                let query = self.overlay.query();
                if !query.is_empty() && query != self.searched {
                    self.searched.clear();
                    self.searched.push_str(query);
                    self.ide.start_search(&self.searched, false, false);
                    self.search_hits = usize::MAX;
                    self.overlay.set_status(String::from("recherche en cours"));
                    return;
                }
            }
            _ => {}
        }
        let Some(picked) = self.overlay.selected().map(describe_row) else {
            self.close_panel();
            return;
        };
        self.take_row(picked);
    }

    fn take_row(&mut self, picked: PickedRow) {
        if picked.command {
            let found = commands::all()
                .iter()
                .copied()
                .find(|command| commands::label(*command) == picked.label);
            self.close_panel();
            if let Some(command) = found {
                self.pending_command = Some(command);
            }
            return;
        }
        let Some((path, cursor)) = picked.target else {
            return;
        };
        self.close_panel();
        self.jump_to(&path, cursor, picked.utf16);
    }

    fn open_project_dialog(&mut self) {
        if self.folder.is_some() {
            self.status_message("dialogue deja ouvert");
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let start = self.ide.project().root().to_path_buf();
        let spawned = std::thread::Builder::new()
            .name(String::from("dialogue"))
            .spawn(move || {
                let chosen = choose_folder(&start);
                let _ = sender.send(chosen);
            });
        match spawned {
            Ok(_) => {
                self.folder = Some(receiver);
                self.status_message("choisis un dossier");
            }
            Err(err) => self.set_status(format!("dialogue: {err}")),
        }
    }

    fn poll_folder(&mut self) {
        let Some(receiver) = self.folder.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Some(path)) => {
                self.folder = None;
                self.adopt_project(&path);
            }
            Ok(None) => {
                self.folder = None;
                self.status_message("ouverture annulee");
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.folder = None,
        }
    }

    fn adopt_project(&mut self, root: &Path) {
        if let Err(err) = self.ide.open_project(root) {
            self.set_status(err);
            return;
        }
        self.close_panel();
        self.indexed = usize::MAX;
        self.targets_seen = usize::MAX;
        self.sync_finder(true);
        self.searched.clear();
        self.search_hits = 0;
        self.symbols.clear();
        self.after_activate();
        let mut message = String::from("projet ");
        message.push_str(self.ide.project().label());
        message.push_str(" (");
        message.push_str(self.ide.project().kind().label());
        message.push(')');
        self.set_status(message);
    }

    fn run_task(&mut self, task: Task) {
        if !self.overlay.output() {
            self.overlay.toggle_output();
        }
        match self.ide.run_task(task) {
            Ok(()) => {
                let mut message = String::from("tache ");
                message.push_str(self.ide.tasks().label());
                self.set_status(message);
            }
            Err(err) => self.set_status(err),
        }
    }

    fn stop_task(&mut self) {
        if !self.ide.tasks().running() {
            self.status_message("aucune tache en cours");
            return;
        }
        self.ide.stop_task();
        self.status_message("tache arretee");
    }

    fn close_active_tab(&mut self) {
        let index = self.ide.workspace().active();
        self.close_tab(index);
    }

    fn close_tab(&mut self, index: usize) {
        let force = self.close_pending == Some(index);
        match self.ide.close_tab(index, force) {
            Ok(()) => {
                self.close_pending = None;
                self.after_activate();
            }
            Err(err) => {
                self.close_pending = Some(index);
                self.set_status(err);
            }
        }
    }

    fn activate_tab(&mut self, index: usize) {
        if index >= self.ide.workspace().len() {
            self.status_message("onglet inexistant");
            return;
        }
        if index == self.ide.workspace().active() {
            return;
        }
        self.ide.workspace_mut().set_scroll(self.scroll);
        self.ide.workspace_mut().activate(index);
        self.after_activate();
    }

    fn cycle_tab(&mut self, forward: bool) {
        if self.ide.workspace().len() < 2 {
            self.status_message("un seul onglet");
            return;
        }
        self.ide.workspace_mut().set_scroll(self.scroll);
        self.ide.workspace_mut().cycle(forward);
        self.after_activate();
    }

    fn jump(&mut self, forward: bool) {
        self.ide.workspace_mut().set_scroll(self.scroll);
        let target = if forward {
            self.ide.workspace_mut().forward()
        } else {
            self.ide.workspace_mut().back()
        };
        let Some((path, cursor)) = target else {
            self.status_message(if forward {
                "aucun saut en avant"
            } else {
                "aucun saut en arriere"
            });
            return;
        };
        self.open_path(&path, Some(cursor));
    }

    fn save_active(&mut self) {
        self.ide.workspace_mut().save_active();
        self.ide.notify_saved();
        self.needs_diagnostics = true;
        let message = self.ide.workspace().buffer().status.clone();
        self.set_status(message);
    }

    fn save_all(&mut self) {
        let count = self.ide.workspace_mut().save_all();
        self.ide.notify_saved();
        self.needs_diagnostics = true;
        if count == 0 {
            self.status_message("rien a enregistrer");
        } else {
            self.set_status(format!("{count} fichiers enregistres"));
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        let dirty = self.ide.workspace().dirty();
        if dirty > 0 && self.close_pending != Some(usize::MAX) {
            self.close_pending = Some(usize::MAX);
            self.set_status(format!(
                "{dirty} fichiers modifies, recommence pour quitter sans enregistrer"
            ));
            return;
        }
        self.ide.shutdown();
        event_loop.exit();
    }

    fn pump_notices(&mut self) {
        let mut drained = std::mem::take(&mut self.notices);
        drained.clear();
        self.ide.poll(&mut drained);
        for notice in drained.drain(..) {
            self.apply_notice(notice);
        }
        self.notices = drained;
    }

    fn apply_notice(&mut self, notice: Notice) {
        match notice {
            Notice::Status(message) => self.set_status(message),
            Notice::Diagnostics(path) => {
                if self.ide.workspace().buffer().path.as_deref() == Some(path.as_path()) {
                    self.needs_diagnostics = true;
                }
                if self.overlay.panel() == Panel::Problems {
                    self.refresh_panel_rows();
                }
            }
            Notice::Completion { items, incomplete } => {
                self.completion_partial = incomplete;
                self.needs_popup = true;
                let App { ide, completion, .. } = self;
                let prefix = ide.workspace().buffer().word_prefix();
                completion.set_language_items(&items, prefix);
            }
            Notice::Hover(text) => {
                self.hover = if text.is_empty() { None } else { Some(text) };
            }
            Notice::Signature(info) => self.signature = info,
            Notice::Definition(locations) => match locations.len() {
                0 => self.status_message("definition introuvable"),
                1 => {
                    let location = &locations[0];
                    let path = location.path.clone();
                    let cursor = Cursor {
                        line: location.range.start.line as usize,
                        column: location.range.start.character as usize,
                    };
                    self.jump_to(&path, cursor, true);
                }
                _ => {
                    self.symbols.clear();
                    self.symbols
                        .extend(locations.iter().map(location_symbol));
                    self.open_panel(Panel::References);
                    self.refresh_panel_rows();
                }
            },
            Notice::References(symbols) => {
                self.symbols = symbols;
                if self.overlay.panel() != Panel::References {
                    self.open_panel(Panel::References);
                }
                self.refresh_panel_rows();
            }
            Notice::DocumentSymbols(symbols) => {
                self.symbols = symbols;
                if self.overlay.panel() == Panel::DocumentSymbols {
                    self.refresh_panel_rows();
                }
            }
            Notice::WorkspaceSymbols(symbols) => {
                self.symbols = symbols;
                if self.overlay.panel() == Panel::WorkspaceSymbols {
                    self.refresh_panel_rows();
                }
            }
            Notice::Renamed { files, edits } => {
                self.set_status(format!("renomme dans {files} fichiers, {edits} editions"));
                self.needs_rebuild = true;
                self.needs_diagnostics = true;
                self.focus_version = 0;
            }
            Notice::Formatted(edits) => {
                self.set_status(format!("formate, {edits} editions"));
                self.needs_rebuild = true;
                self.needs_diagnostics = true;
                self.focus_version = 0;
            }
            Notice::TaskFinished { label, code } => {
                match code {
                    Some(0) => self.set_status(format!("{label} termine")),
                    Some(code) => self.set_status(format!("{label} echoue (code {code})")),
                    None => self.set_status(format!("{label} interrompu")),
                }
                self.needs_diagnostics = true;
                self.ide.project_mut().refresh();
                if self.overlay.panel() == Panel::Problems {
                    self.refresh_panel_rows();
                }
            }
            Notice::Failed(message) => self.set_status(message),
        }
    }

    fn rebuild_diagnostics(&mut self) {
        self.spans.clear();
        self.gutter.clear();
        {
            let App { ide, spans, gutter, .. } = self;
            let buffer = ide.workspace().buffer();
            if let Some(path) = buffer.path.as_deref() {
                for diagnostic in ide.diagnostics_for(path) {
                    let first = diagnostic.range.start.line as usize;
                    let last = (diagnostic.range.end.line as usize)
                        .max(first)
                        .min(first + SPAN_LINES);
                    if first >= buffer.line_count() {
                        continue;
                    }
                    match gutter.last_mut() {
                        Some(mark) if mark.line == first => {
                            if severity_rank(diagnostic.severity) < severity_rank(mark.severity) {
                                mark.severity = diagnostic.severity;
                            }
                        }
                        _ => gutter.push(GutterMark {
                            line: first,
                            severity: diagnostic.severity,
                        }),
                    }
                    for line in first..=last.min(buffer.line_count().saturating_sub(1)) {
                        if spans.len() >= SPAN_LIMIT {
                            break;
                        }
                        let width = buffer.line_chars(line);
                        let start = if line == first {
                            (diagnostic.range.start.character as usize).min(width)
                        } else {
                            0
                        };
                        let end = if line == diagnostic.range.end.line as usize {
                            (diagnostic.range.end.character as usize).min(width)
                        } else {
                            width
                        };
                        let end = if end > start { end } else { (start + 1).min(width.max(1)) };
                        spans.push(DiagnosticSpan {
                            line,
                            start,
                            end,
                            severity: diagnostic.severity,
                        });
                    }
                }
            }
        }
        let App { renderer, spans, gutter, .. } = self;
        if let Some(renderer) = renderer.as_mut() {
            renderer.set_diagnostics(spans);
            renderer.set_gutter(gutter);
        }
    }

    fn build_hud(&mut self) {
        let view = match self.renderer.as_ref() {
            Some(renderer) => renderer.hud_viewport(&self.camera),
            None => return,
        };
        self.sync_overlay_rows(view.width, view.height);
        let App { renderer, hud, ide, overlay, font, hover, signature, .. } = self;
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        {
            let workspace = ide.workspace();
            let model = HudModel {
                project: ide.project(),
                tabs: workspace.tabs(),
                active_tab: workspace.active(),
                overlay,
                tasks: ide.tasks(),
                search: ide.search(),
                server_status: ide.server_status(),
                diagnostics: ide.diagnostic_counts(),
                cursor: workspace.buffer().cursor(),
                language: workspace.language(),
                hover: hover.as_deref(),
                signature: signature.as_ref(),
                font_name: font.name(),
            };
            hud.build(view, &model);
        }
        renderer.set_hud(hud, font);
    }

    fn poll_hover(&mut self) {
        let Some(since) = self.hover_since else {
            return;
        };
        if since.elapsed() < HOVER_DELAY {
            return;
        }
        self.hover_since = None;
        if !self.ide.server_ready() || self.overlay.is_capturing_input() {
            return;
        }
        self.ide.ask_hover();
    }

    fn sync_overlay_rows(&mut self, width: f32, height: f32) {
        let height = height.max(hud::TAB_BAR_HEIGHT + hud::STATUS_HEIGHT + 1.0);
        let status_top = height - hud::STATUS_HEIGHT;
        let output_top = if self.overlay.output() {
            (status_top - hud::OUTPUT_HEIGHT).max(hud::TAB_BAR_HEIGHT)
        } else {
            status_top
        };
        if self.overlay.sidebar() && width > 0.0 {
            let tree = ((output_top - hud::TAB_BAR_HEIGHT).max(0.0) / hud::TREE_ROW_HEIGHT)
                .floor()
                .max(1.0) as usize;
            self.overlay.set_tree_rows(tree);
        }
        if !self.overlay.output() {
            return;
        }
        let list = ((status_top - output_top - hud::OUTPUT_HEADER_HEIGHT).max(0.0)
            / hud::OUTPUT_ROW_HEIGHT)
            .floor()
            .max(1.0) as usize;
        self.overlay.set_output_rows(list);
        let total = self.ide.tasks().lines().len();
        self.overlay.follow_output(total);
    }

    fn poll_targets(&mut self) {
        let count = self.ide.project().targets().len();
        if count == self.targets_seen {
            return;
        }
        self.targets_seen = count;
        if count == 0 {
            let failure = self.ide.project().targets_error().map(String::from);
            if let Some(failure) = failure {
                self.set_status(failure);
            }
            return;
        }
        let mut message = String::with_capacity(64);
        let _ = write!(message, "{count} cibles: ");
        for (index, target) in self.ide.project().targets().iter().take(4).enumerate() {
            if index > 0 {
                message.push_str(", ");
            }
            message.push_str(&target.name);
            message.push(' ');
            message.push_str(&target.kind);
        }
        self.set_status(message);
    }

    fn poll_line_diagnostic(&mut self) {
        let line = self.ide.workspace().buffer().cursor_line;
        if self.diagnostic_line == Some(line) {
            return;
        }
        self.diagnostic_line = Some(line);
        let message = {
            let buffer = self.ide.workspace().buffer();
            let Some(path) = buffer.path.as_deref() else {
                return;
            };
            let found = self.ide.diagnostics_on_line(path, line);
            let Some(first) = found.first() else {
                return;
            };
            String::from(first.message.lines().next().unwrap_or(""))
        };
        if !message.is_empty() {
            self.set_status(message);
        }
    }

    fn poll_search(&mut self) {
        if self.overlay.panel() != Panel::Search {
            return;
        }
        let hits = self.ide.search().hits().len();
        let running = self.ide.search().running();
        if hits == self.search_hits && running == self.search_running {
            return;
        }
        self.search_hits = hits;
        self.search_running = running;
        self.refresh_panel_rows();
    }

    fn handle_find_key(&mut self, event: &KeyEvent, chord: Chord) -> bool {
        let command = chord.command;
        let shift = chord.shift;
        match event.logical_key.as_ref() {
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
                    self.run_find();
                }
                true
            }
            _ => {
                if command || chord.control || chord.alt {
                    match commands::letter_of(&event.logical_key) {
                        Some('f') if command => {
                            self.find.replacing = false;
                            self.needs_find_bar = true;
                        }
                        Some('g') if command => self.step_match(!shift),
                        Some('v') if command => self.paste_into_find(),
                        _ => return false,
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
                    self.run_find();
                }
                true
            }
        }
    }

    fn handle_panel_key(&mut self, event: &KeyEvent, chord: Chord, event_loop: &ActiveEventLoop) {
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.close_panel(),
            Key::Named(NamedKey::Enter) => self.accept_panel(),
            Key::Named(NamedKey::ArrowUp) => self.overlay.move_selection(-1),
            Key::Named(NamedKey::ArrowDown) => self.overlay.move_selection(1),
            Key::Named(NamedKey::PageUp) => self.overlay.move_selection(-(VISIBLE_ROWS as isize)),
            Key::Named(NamedKey::PageDown) => self.overlay.move_selection(VISIBLE_ROWS as isize),
            Key::Named(NamedKey::Tab) => {
                self.overlay.move_selection(if chord.shift { -1 } else { 1 })
            }
            Key::Named(NamedKey::Backspace) => {
                self.overlay.backspace();
                self.on_query_changed();
            }
            _ => {
                if chord.command || chord.control || chord.alt {
                    if let Some(command) =
                        commands::resolve(&event.logical_key, event.physical_key, chord)
                    {
                        commands::execute(self, command, event_loop);
                        return;
                    }
                    if chord.command && commands::letter_of(&event.logical_key) == Some('v') {
                        self.paste_into_panel();
                    }
                    return;
                }
                let Some(written) = event.text.as_ref() else {
                    return;
                };
                let mut changed = false;
                for ch in written.chars() {
                    if !ch.is_control() {
                        self.overlay.insert(ch);
                        changed = true;
                    }
                }
                if changed {
                    self.on_query_changed();
                }
            }
        }
    }

    fn paste_into_panel(&mut self) {
        let Some(clip) = clipboard_read() else {
            return;
        };
        let line = clip.lines().next().unwrap_or("");
        if line.is_empty() {
            return;
        }
        for ch in line.chars() {
            if !ch.is_control() {
                self.overlay.insert(ch);
            }
        }
        self.on_query_changed();
    }

    fn handle_command_key(&mut self, event: &KeyEvent, chord: Chord) {
        let shift = chord.shift;
        let alt = chord.alt;
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Space) => self.update_completion(true),
            Key::Named(NamedKey::ArrowLeft) if alt => {
                self.ide.workspace_mut().buffer_mut().move_word(false, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowRight) if alt => {
                self.ide.workspace_mut().buffer_mut().move_word(true, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.ide.workspace_mut().buffer_mut().move_home(shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.ide.workspace_mut().buffer_mut().move_end(shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::Home) => {
                self.ide
                    .workspace_mut()
                    .buffer_mut()
                    .move_document(false, shift);
                self.refresh_view();
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::End) => {
                self.ide
                    .workspace_mut()
                    .buffer_mut()
                    .move_document(true, shift);
                self.refresh_view();
                self.close_popup();
            }
            _ => match commands::letter_of(&event.logical_key) {
                Some('p') if shift => self.open_commands(),
                Some('a') => self.select_everything(),
                Some('c') => self.copy(),
                Some('x') => self.cut(),
                Some('v') => self.paste(),
                Some('z') if shift => self.redo_edit(),
                Some('z') => self.undo_edit(),
                Some('f') => self.open_find(),
                Some('g') => self.step_match(!shift),
                _ => {}
            },
        }
    }

    fn handle_control_key(&mut self, event: &KeyEvent) {
        if matches!(event.logical_key.as_ref(), Key::Named(NamedKey::Space)) {
            self.update_completion(true);
            if self.ide.server_ready() {
                self.ide.ask_completion(None);
            }
        }
    }

    fn handle_alt_key(&mut self, event: &KeyEvent, chord: Chord) {
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowLeft) if chord.shift => {
                self.ide.workspace_mut().buffer_mut().move_word(false, true);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowRight) if chord.shift => {
                self.ide.workspace_mut().buffer_mut().move_word(true, true);
                self.refresh_view();
            }
            Key::Named(NamedKey::Backspace) => {
                self.ide.workspace_mut().buffer_mut().delete_word_left();
                self.after_edit();
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
        let chord = Chord::from_state(self.modifiers);
        if self.overlay.is_capturing_input() {
            self.handle_panel_key(event, chord, event_loop);
            return;
        }
        if let Some(command) = commands::resolve(&event.logical_key, event.physical_key, chord) {
            commands::execute(self, command, event_loop);
            return;
        }
        if self.find.open && self.handle_find_key(event, chord) {
            return;
        }
        if chord.command && !chord.control {
            self.handle_command_key(event, chord);
            return;
        }
        if chord.control && !chord.command {
            self.handle_control_key(event);
            return;
        }
        if chord.alt && !chord.command && !chord.control {
            self.handle_alt_key(event, chord);
            return;
        }

        if self.completion.active {
            match event.logical_key.as_ref() {
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

        let shift = chord.shift;
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Backspace) => {
                self.ide.workspace_mut().buffer_mut().backspace();
                self.after_edit();
                self.update_completion(false);
            }
            Key::Named(NamedKey::Delete) => {
                self.ide.workspace_mut().buffer_mut().delete();
                self.after_edit();
                self.update_completion(false);
            }
            Key::Named(NamedKey::Enter) => {
                self.ide.workspace_mut().buffer_mut().insert_newline();
                self.after_edit();
                self.close_popup();
            }
            Key::Named(NamedKey::Tab) => {
                let empty = self.ide.workspace().buffer().word_prefix().is_empty();
                if !empty {
                    self.update_completion(true);
                }
                if !self.completion.active {
                    self.ide.workspace_mut().buffer_mut().insert_str("    ");
                    self.after_edit();
                }
            }
            Key::Named(NamedKey::Escape) => {
                if self.find.open {
                    self.close_find();
                } else if self.ide.workspace().buffer().selection().is_some() {
                    self.ide.workspace_mut().buffer_mut().clear_selection();
                    self.refresh_view();
                }
                self.close_popup();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.ide.workspace_mut().buffer_mut().move_left(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.ide.workspace_mut().buffer_mut().move_right(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.ide.workspace_mut().buffer_mut().move_vertical(-1, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.ide.workspace_mut().buffer_mut().move_vertical(1, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::PageUp) => {
                self.ide
                    .workspace_mut()
                    .buffer_mut()
                    .move_vertical(-20, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::PageDown) => {
                self.ide.workspace_mut().buffer_mut().move_vertical(20, shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::Home) => {
                self.ide.workspace_mut().buffer_mut().move_home(shift);
                self.refresh_view();
            }
            Key::Named(NamedKey::End) => {
                self.ide.workspace_mut().buffer_mut().move_end(shift);
                self.refresh_view();
            }
            _ => {
                let Some(written) = event.text.as_ref() else {
                    return;
                };
                let mut typed = None;
                for ch in written.chars() {
                    if ch.is_control() {
                        continue;
                    }
                    let previous = self.previous_char();
                    self.ide.workspace_mut().buffer_mut().insert_char(ch);
                    typed = Some((previous, ch));
                }
                if let Some((previous, ch)) = typed {
                    self.after_edit();
                    self.update_completion(false);
                    self.ask_language_completion(previous, ch);
                }
            }
        }
    }

    fn previous_char(&self) -> Option<char> {
        let buffer = self.ide.workspace().buffer();
        let line = buffer.lines.get(buffer.cursor_line)?;
        if buffer.cursor_col == 0 {
            return None;
        }
        line.chars().nth(buffer.cursor_col - 1)
    }

    fn run_pending(&mut self, event_loop: &ActiveEventLoop) {
        let Some(command) = self.pending_command.take() else {
            return;
        };
        commands::execute(self, command, event_loop);
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let elapsed = (now - self.start).as_secs_f32();

        if self.renderer.is_none() {
            return;
        }

        self.poll_folder();
        self.pump_notices();
        self.poll_search();
        self.poll_targets();
        self.poll_hover();

        if !self.status.is_empty() {
            match self.status_since {
                None => self.status_since = Some(now),
                Some(since) if (now - since).as_secs_f32() > STATUS_LIFETIME => {
                    self.status.clear();
                    self.status_since = None;
                }
                Some(_) => {}
            }
        }

        self.build_hud();
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
            self.sync_shading();
            {
                let App { renderer, ide, font, camera, .. } = self;
                if let Some(renderer) = renderer.as_mut() {
                    let workspace = ide.workspace();
                    renderer.rebuild(workspace.buffer(), font, workspace.highlighter(), camera);
                }
            }
            self.rebuild_eye = eye;
            self.rebuild_distance = self.camera.distance.max(1.0);
        }

        if self.needs_diagnostics {
            self.needs_diagnostics = false;
            self.rebuild_diagnostics();
        }

        if self.needs_popup {
            self.needs_popup = false;
            let App { renderer, ide, font, completion, .. } = self;
            if let Some(renderer) = renderer.as_mut() {
                renderer.set_popup(completion, ide.workspace().buffer(), font);
            }
        }

        if self.needs_find_bar {
            self.needs_find_bar = false;
            let App { renderer, ide, font, find, .. } = self;
            if let Some(renderer) = renderer.as_mut() {
                let buffer = ide.workspace().buffer();
                let bar = if find.open {
                    Some(FindBarView {
                        query: &find.query,
                        replacement: &find.replacement,
                        replacing: find.replacing,
                        current: buffer.current_match(),
                        total: buffer.matches().len(),
                    })
                } else {
                    None
                };
                renderer.set_find_bar(bar, buffer, font);
            }
        }

        self.poll_line_diagnostic();

        self.refresh_title();

        let wave = self.wave;
        let grid = self.grid;
        let outcome = match self.renderer.as_mut() {
            Some(renderer) => renderer.render(&self.camera, elapsed, wave, grid),
            None => return,
        };
        if let Err(err) = outcome
            && self.status != err
        {
            self.set_status(err);
        }
    }
}

struct PickedRow {
    label: String,
    command: bool,
    utf16: bool,
    target: Option<(PathBuf, Cursor)>,
}

fn describe_row(row: &Row) -> PickedRow {
    PickedRow {
        label: row.label.clone(),
        command: matches!(row.kind, RowKind::Command),
        utf16: matches!(row.kind, RowKind::Symbol | RowKind::Reference),
        target: row.target.clone(),
    }
}

fn location_symbol(location: &lsp::protocol::Location) -> Symbol {
    let name = location
        .path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    Symbol {
        name,
        kind: 0,
        container: String::new(),
        range: location.range,
        selection: location.range,
        path: location.path.clone(),
        depth: 0,
    }
}

fn symbol_tag(kind: u32) -> &'static str {
    match kind {
        2 => "mod",
        3 => "esp",
        4 => "paq",
        5 => "cl",
        6 => "me",
        7 => "pr",
        8 => "ch",
        9 => "ini",
        10 => "en",
        11 => "tr",
        12 => "fn",
        13 => "var",
        14 => "cst",
        15 => "txt",
        16 => "num",
        23 => "st",
        26 => "ty",
        _ => "sy",
    }
}

fn severity_kind(severity: Severity) -> RowKind {
    match severity {
        Severity::Error => RowKind::Error,
        Severity::Warning => RowKind::Warning,
        Severity::Information => RowKind::Information,
        Severity::Hint => RowKind::Hint,
    }
}

fn severity_tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "err",
        Severity::Warning => "avt",
        Severity::Information => "inf",
        Severity::Hint => "ind",
    }
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Information => 2,
        Severity::Hint => 3,
    }
}

fn choose_folder(start: &Path) -> Option<PathBuf> {
    let mut script = String::with_capacity(160);
    script.push_str("POSIX path of (choose folder with prompt \"ouvrir un projet\"");
    if start.is_dir() {
        script.push_str(" default location POSIX file ");
        applescript_string(&start.to_string_lossy(), &mut script);
    }
    script.push(')');
    let output = Process::new("osascript").arg("-e").arg(&script).output();
    let output = output.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

fn applescript_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
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
        self.build_hud();
        self.update_focus();
        self.camera.snap();
        self.refresh_title();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.dispatch(event_loop, event);
        self.run_pending(event_loop);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.ide.shutdown();
    }
}

impl App {
    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop),
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
                self.wheel(amount);
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }
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
    let output = Process::new("pbpaste").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn clipboard_write(text: &str) {
    let Ok(mut child) = Process::new("pbcopy").stdin(Stdio::piped()).spawn() else {
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

fn startup() -> (Project, Workspace, Option<PathBuf>) {
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    let Some(argument) = argument else {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project = Project::open(&root);
        let workspace = Workspace::new(TextBuffer::from_str(DEMO, None));
        return (project, workspace, None);
    };
    let project = Project::open(&argument);
    if argument.is_dir() {
        let workspace = Workspace::new(TextBuffer::from_str(DEMO, None));
        return (project, workspace, None);
    }
    let workspace = Workspace::new(TextBuffer::from_str("", None));
    (project, workspace, Some(argument))
}

fn main() {
    let font = match Font::load() {
        Ok(font) => font,
        Err(err) => {
            eprintln!("fonte introuvable: {err}");
            std::process::exit(1);
        }
    };

    let (project, workspace, file) = startup();
    let mut ide = Ide::new(project, workspace);
    let mut opening = None;
    if let Some(file) = file
        && let Err(err) = ide.open_file(&file, None)
    {
        opening = Some(err);
    }

    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("boucle d evenements: {err}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(font, ide);
    app.sync_finder(true);
    if let Some(err) = opening {
        app.set_status(err);
    } else {
        app.after_activate();
    }
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("execution: {err}");
    }
}
