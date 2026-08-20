mod atlas;
mod camera;
mod complete;
mod extrude;
mod font;
mod render;
mod syntax;
mod text;

#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use camera::Camera;
use complete::{Completion, MIN_PREFIX};
use font::Font;
use render::Renderer;
use syntax::{Highlighter, Language};
use text::TextBuffer;

const BLINK_PERIOD: f32 = 1.15;
const EDGE_PADDING: f32 = 1.2;
const STATUS_LIFETIME: f32 = 4.0;
const DEMO: &str = "\
bloc-notes en trois dimensions
------------------------------

chaque lettre est un maillage extrude,
tessele depuis les contours de la fonte
puis rendu en instancie sur le GPU.

ouvre un .rs ou un .cs : tree-sitter colore
la syntaxe et la completion propose les
symboles du fichier, les mots-cles et les
mots du tampon, classes en fuzzy.

  glisser gauche    tourner autour du texte
  glisser droit     translater
  molette           avancer / reculer
  option + fleches  tourner au clavier
  F1                recadrer sur le curseur
  F2                onduler ou figer le texte
  F3                afficher ou masquer la grille
  cmd + S           enregistrer
  cmd + C / X / V   copier, couper, coller la ligne
  tab / ctrl+espace ouvrir la completion
  haut / bas        parcourir, entree valide, echap ferme
  cmd + debut/fin   haut ou bas du document

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
    last: (f32, f32),
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
    language: Option<Language>,
    highlighter: Option<Highlighter>,
    completion: Completion,
    status_since: Option<Instant>,
    scroll: (f32, f32),
    title: String,
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
            pointer: Pointer { orbiting: false, panning: false, last: (0.0, 0.0) },
            modifiers: ModifiersState::empty(),
            start: Instant::now(),
            last_frame: Instant::now(),
            needs_rebuild: true,
            needs_popup: true,
            status_since: None,
            scroll: (0.0, 0.0),
            title: String::new(),
        }
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
        self.needs_rebuild = true;
        self.needs_popup = true;
    }

    fn update_focus(&mut self) {
        let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
        let (half_width, half_height) = self.camera.half_extent(aspect);
        let cursor_x = self.text.cursor_col as f32 * self.font.advance();
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

    fn refresh_title(&mut self) {
        let name = self
            .text
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("sans titre"));
        let mark = if self.text.modified { " *" } else { "" };
        let mut status = match self.language {
            Some(language) => format!("  |  {}", language.label()),
            None => String::new(),
        };
        if !self.text.status.is_empty() {
            status.push_str("  |  ");
            status.push_str(&self.text.status);
        }
        let title = format!(
            "text3d  |  {}{}  |  {}:{}{}",
            name,
            mark,
            self.text.cursor_line + 1,
            self.text.cursor_col + 1,
            status
        );
        if title != self.title {
            if let Some(window) = &self.window {
                window.set_title(&title);
            }
            self.title = title;
        }
    }

    fn close_popup(&mut self) {
        self.completion.dismiss();
        self.needs_popup = true;
    }

    fn handle_key(&mut self, event: &winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }
        let command = self.modifiers.super_key() || self.modifiers.control_key();

        if command {
            match event.logical_key.as_ref() {
                Key::Character("s") => {
                    self.text.save();
                    self.status_since = Some(Instant::now());
                    self.refresh_title();
                }
                Key::Character("q") => event_loop.exit(),
                Key::Character("v") => {
                    if let Some(clip) = clipboard_read() {
                        self.text.insert_text(&clip);
                        self.needs_rebuild = true;
                        self.close_popup();
                    }
                }
                Key::Character("c") => {
                    clipboard_write(self.text.current_line());
                    self.text.status = String::from("ligne copiee");
                    self.status_since = Some(Instant::now());
                    self.refresh_title();
                }
                Key::Character("x") => {
                    clipboard_write(self.text.current_line());
                    self.text.delete_line();
                    self.text.status = String::from("ligne coupee");
                    self.status_since = Some(Instant::now());
                    self.needs_rebuild = true;
                    self.close_popup();
                }
                Key::Named(NamedKey::Space) => self.update_completion(true),
                Key::Named(NamedKey::Home) => {
                    self.text.move_document(false);
                    self.needs_rebuild = true;
                    self.close_popup();
                }
                Key::Named(NamedKey::End) => {
                    self.text.move_document(true);
                    self.needs_rebuild = true;
                    self.close_popup();
                }
                _ => {}
            }
            return;
        }

        if self.modifiers.alt_key() {
            match &event.logical_key {
                Key::Named(NamedKey::ArrowLeft) => self.camera.orbit(-40.0, 0.0),
                Key::Named(NamedKey::ArrowRight) => self.camera.orbit(40.0, 0.0),
                Key::Named(NamedKey::ArrowUp) => self.camera.orbit(0.0, -40.0),
                Key::Named(NamedKey::ArrowDown) => self.camera.orbit(0.0, 40.0),
                Key::Named(NamedKey::PageUp) => self.camera.zoom(1.0),
                Key::Named(NamedKey::PageDown) => self.camera.zoom(-1.0),
                _ => {}
            }
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

        match &event.logical_key {
            Key::Named(NamedKey::Backspace) => {
                self.text.backspace();
                self.needs_rebuild = true;
                self.update_completion(false);
            }
            Key::Named(NamedKey::Delete) => {
                self.text.delete();
                self.needs_rebuild = true;
                self.update_completion(false);
            }
            Key::Named(NamedKey::Enter) => {
                self.text.insert_newline();
                self.needs_rebuild = true;
                self.close_popup();
            }
            Key::Named(NamedKey::Tab) => {
                if !self.text.word_prefix().is_empty() {
                    self.update_completion(true);
                }
                if !self.completion.active {
                    self.text.insert_str("    ");
                    self.needs_rebuild = true;
                }
            }
            Key::Named(NamedKey::Escape) => self.close_popup(),
            Key::Named(NamedKey::ArrowLeft) => {
                self.text.move_left();
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.text.move_right();
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.text.move_vertical(-1);
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.text.move_vertical(1);
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::PageUp) => {
                self.text.move_vertical(-20);
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::PageDown) => {
                self.text.move_vertical(20);
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::Home) => {
                self.text.move_home();
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::End) => {
                self.text.move_end();
                self.needs_rebuild = true;
            }
            Key::Named(NamedKey::F1) => self.camera.reset_orientation(),
            Key::Named(NamedKey::F2) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.wave = if renderer.wave > 0.0 { 0.0 } else { 0.014 };
                }
            }
            Key::Named(NamedKey::F3) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.show_grid = !renderer.show_grid;
                }
            }
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
                        self.needs_rebuild = true;
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

        if self.needs_rebuild {
            self.needs_rebuild = false;
            self.needs_popup = true;
            self.text.sync();
            if let Some(highlighter) = self.highlighter.as_mut() {
                let window = self.text.byte_range(render::visible_lines(&self.text));
                highlighter.update(self.text.source(), self.text.line_starts(), window);
            }
            let highlighter = self.highlighter.as_ref();
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.rebuild(&self.text, &self.font, highlighter);
            }
            self.refresh_title();
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

        if !self.text.status.is_empty() {
            match self.status_since {
                None => self.status_since = Some(now),
                Some(since) if (now - since).as_secs_f32() > STATUS_LIFETIME => {
                    self.text.status.clear();
                    self.status_since = None;
                    self.refresh_title();
                }
                Some(_) => {}
            }
        }

        self.update_focus();
        self.camera.update(dt);
        let cursor_y = -(self.text.cursor_line as f32) * self.font.line_height();
        let blink = (elapsed / BLINK_PERIOD).fract() < 0.62;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.render(&self.camera, &self.font, cursor_y, elapsed, blink);
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
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event, event_loop),
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => {
                        if self.modifiers.shift_key() {
                            self.pointer.panning = pressed;
                        } else {
                            self.pointer.orbiting = pressed;
                        }
                    }
                    MouseButton::Right | MouseButton::Middle => self.pointer.panning = pressed,
                    _ => {}
                }
                if !pressed {
                    self.pointer.orbiting = false;
                    self.pointer.panning = false;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let current = (position.x as f32, position.y as f32);
                let delta = (current.0 - self.pointer.last.0, current.1 - self.pointer.last.1);
                self.pointer.last = current;
                if self.pointer.orbiting {
                    self.camera.orbit(delta.0, delta.1);
                } else if self.pointer.panning {
                    let height = self
                        .renderer
                        .as_ref()
                        .map(|r| r.viewport_height())
                        .unwrap_or(800.0);
                    self.camera.pan_by(delta.0, delta.1, height);
                }
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
