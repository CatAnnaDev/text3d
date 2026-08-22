use crate::lsp::protocol::SignatureInfo;
use crate::overlay::{Overlay, Panel, Row};
use crate::project::Project;
use crate::search::ProjectSearch;
use crate::syntax::Language;
use crate::tasks::{Stream, TaskRunner};
use crate::text::Cursor;
use crate::workspace::Tab;
use std::fmt::Write;

pub const CHAR_WIDTH: f32 = 0.6;
pub const TAB_BAR_HEIGHT: f32 = 1.8;
pub const SIDEBAR_WIDTH: f32 = 26.0;
pub const STATUS_HEIGHT: f32 = 1.5;
pub const OUTPUT_HEIGHT: f32 = 14.0;
pub const OUTPUT_HEADER_HEIGHT: f32 = 1.4;
pub const PALETTE_WIDTH: f32 = 64.0;
pub const PALETTE_TOP: f32 = 4.0;
pub const TREE_ROW_HEIGHT: f32 = 1.3;
pub const PANEL_ROW_HEIGHT: f32 = 1.4;
pub const OUTPUT_ROW_HEIGHT: f32 = 1.2;
pub const TAB_CLOSE_WIDTH: f32 = 1.2;
pub const TREE_INDENT: f32 = 1.2;
pub const CHEVRON_WIDTH: f32 = 1.2;
pub const SCROLLBAR_WIDTH: f32 = 0.4;
pub const CARD_MAX_WIDTH: f32 = 70.0;

const TAB_PAD: f32 = 0.6;
const TAB_GAP: f32 = 0.35;
const TAB_MODIFIED_WIDTH: f32 = 0.7;
const TAB_NAME_MAX: usize = 18;
const TREE_PAD: f32 = 0.5;
const PANEL_PAD: f32 = 0.6;
const PANEL_EDGE_WIDTH: f32 = 0.12;
const STATUS_PAD: f32 = 0.7;
const STATUS_GAP: f32 = 1.2;
const EDGE_THICKNESS: f32 = 0.08;
const CAP_HEIGHT: f32 = 0.7;
const CARET_WIDTH: f32 = 0.09;
const CARD_ROW_HEIGHT: f32 = 1.25;
const CARD_PAD: f32 = 0.6;
const CARD_GAP: f32 = 1.0;
const CARD_MAX_LINES: usize = 14;
const CARD_MIN_WIDTH: f32 = 14.0;
const DETAIL_FRACTION: f32 = 0.42;
const ELLIPSIS: &str = "..";
const ELLIPSIS_CHARS: usize = 2;

const LAYER_BAR: u8 = 0;
const LAYER_BAR_TEXT: u8 = 1;
const LAYER_CARD: u8 = 2;
const LAYER_CARD_TEXT: u8 = 3;
const LAYER_PANEL: u8 = 4;
const LAYER_PANEL_TEXT: u8 = 5;

const BAR_FILL: [u8; 4] = [1, 2, 4, 250];
const BAR_EDGE: [u8; 4] = [9, 16, 35, 220];
const SIDEBAR_FILL: [u8; 4] = [1, 2, 3, 244];
const TAB_FILL: [u8; 4] = [2, 3, 7, 236];
const TAB_ACTIVE_FILL: [u8; 4] = [4, 6, 16, 244];
const TAB_ACCENT: [u8; 4] = [34, 146, 233, 255];
const PANEL_FILL: [u8; 4] = [1, 2, 4, 252];
const PANEL_EDGE: [u8; 4] = [16, 38, 78, 235];
const PANEL_SELECTED: [u8; 4] = [11, 30, 67, 235];
const CARD_FILL: [u8; 4] = [2, 2, 5, 250];
const CARD_EDGE: [u8; 4] = [13, 27, 59, 230];
const TEXT_MAIN: [u8; 4] = [226, 234, 248, 255];
const TEXT_DIM: [u8; 4] = [84, 96, 122, 255];
const TEXT_FAINT: [u8; 4] = [47, 55, 74, 255];
const TEXT_ACCENT: [u8; 4] = [102, 199, 245, 255];
const DIR_COLOR: [u8; 4] = [156, 190, 232, 255];
const MODIFIED_COLOR: [u8; 4] = [235, 178, 84, 255];
const ERROR_COLOR: [u8; 4] = [232, 104, 104, 255];
const WARNING_COLOR: [u8; 4] = [230, 176, 72, 255];
const RUNNING_COLOR: [u8; 4] = [126, 210, 160, 255];
const STDERR_COLOR: [u8; 4] = [228, 148, 128, 255];
const STDOUT_COLOR: [u8; 4] = [198, 208, 228, 255];
const CARET_COLOR: [u8; 4] = [255, 194, 102, 255];
const SCROLLBAR_TRACK: [u8; 4] = [2, 3, 8, 200];
const SCROLLBAR_THUMB: [u8; 4] = [19, 32, 67, 220];
const SELECTED_ROW: [u8; 4] = [5, 11, 27, 220];

pub struct HudViewport {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub rect: Rect,
    pub color: [u8; 4],
    pub layer: u8,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub color: [u8; 4],
    pub layer: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    None,
    Editor,
    TreeRow(usize),
    TreeToggle(usize),
    Tab(usize),
    TabClose(usize),
    PanelRow(usize),
    OutputLine(usize),
    StatusDiagnostics,
    StatusServer,
    Scrollbar(Region),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Tree,
    Panel,
    Output,
}

pub struct HudModel<'a> {
    pub project: &'a Project,
    pub tabs: &'a [Tab],
    pub active_tab: usize,
    pub overlay: &'a Overlay,
    pub tasks: &'a TaskRunner,
    pub search: &'a ProjectSearch,
    pub server_status: &'a str,
    pub diagnostics: (usize, usize),
    pub cursor: Cursor,
    pub language: Option<Language>,
    pub hover: Option<&'a str>,
    pub signature: Option<&'a SignatureInfo>,
    pub font_name: &'a str,
}

pub struct Hud {
    quads: Vec<Quad>,
    quad_count: usize,
    labels: Vec<Label>,
    label_count: usize,
    hits: Vec<(Rect, Target)>,
    hit_count: usize,
    regions: Vec<(Rect, Region)>,
    region_count: usize,
    editor: Rect,
    scratch: String,
    wrap: Vec<(usize, usize)>,
}

impl Default for Hud {
    fn default() -> Hud {
        Hud::new()
    }
}

impl Hud {
    pub fn new() -> Hud {
        Hud {
            quads: Vec::with_capacity(256),
            quad_count: 0,
            labels: Vec::with_capacity(256),
            label_count: 0,
            hits: Vec::with_capacity(128),
            hit_count: 0,
            regions: Vec::with_capacity(8),
            region_count: 0,
            editor: Rect::new(0.0, 0.0, 0.0, 0.0),
            scratch: String::with_capacity(256),
            wrap: Vec::with_capacity(64),
        }
    }

    pub fn quads(&self) -> &[Quad] {
        &self.quads[..self.quad_count]
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels[..self.label_count]
    }

    pub fn editor_rect(&self) -> Rect {
        self.editor
    }

    pub fn hit(&self, x: f32, y: f32) -> Target {
        for &(rect, target) in self.hits[..self.hit_count].iter().rev() {
            if rect.contains(x, y) {
                return target;
            }
        }
        if self.editor.contains(x, y) {
            return Target::Editor;
        }
        Target::None
    }

    pub fn region_at(&self, x: f32, y: f32) -> Option<Region> {
        for &(rect, region) in self.regions[..self.region_count].iter().rev() {
            if rect.contains(x, y) {
                return Some(region);
            }
        }
        None
    }

    pub fn build(&mut self, view: HudViewport, model: &HudModel) {
        self.quad_count = 0;
        self.label_count = 0;
        self.hit_count = 0;
        self.region_count = 0;

        let width = view.width.max(1.0);
        let height = view.height.max(TAB_BAR_HEIGHT + STATUS_HEIGHT + 1.0);
        let status_top = height - STATUS_HEIGHT;
        let output_top = if model.overlay.output() {
            (status_top - OUTPUT_HEIGHT).max(TAB_BAR_HEIGHT)
        } else {
            status_top
        };
        let sidebar_width = if model.overlay.sidebar() {
            SIDEBAR_WIDTH.min(width * 0.5)
        } else {
            0.0
        };
        self.editor = Rect::new(
            sidebar_width,
            TAB_BAR_HEIGHT,
            (width - sidebar_width).max(0.0),
            (output_top - TAB_BAR_HEIGHT).max(0.0),
        );

        self.build_tabs(width, model);
        if sidebar_width > 0.0 {
            self.build_tree(sidebar_width, TAB_BAR_HEIGHT, output_top, model);
        }
        if model.overlay.output() {
            self.build_output(width, output_top, status_top, model);
        }
        self.build_status(width, status_top, model);
        if model.overlay.panel() == Panel::None {
            self.build_cards(model);
        } else {
            self.build_palette(width, height, model);
        }
    }

    fn build_tabs(&mut self, width: f32, model: &HudModel) {
        let bar = Rect::new(0.0, 0.0, width, TAB_BAR_HEIGHT);
        self.push_quad(bar, BAR_FILL, LAYER_BAR);
        self.push_quad(
            Rect::new(0.0, TAB_BAR_HEIGHT - EDGE_THICKNESS, width, EDGE_THICKNESS),
            BAR_EDGE,
            LAYER_BAR,
        );

        let mut total = 0.0;
        let mut active_start = 0.0;
        let mut active_end = 0.0;
        for (index, tab) in model.tabs.iter().enumerate() {
            let span = tab_span(&tab.name);
            if index == model.active_tab {
                active_start = total;
                active_end = total + span;
            }
            total += span;
        }
        let mut offset = 0.0;
        if total > width {
            if active_end > width {
                offset = active_end - width;
            }
            if active_start < offset {
                offset = active_start;
            }
            offset = offset.clamp(0.0, total - width);
        }

        let mut x = -offset;
        for (index, tab) in model.tabs.iter().enumerate() {
            let span = tab_span(&tab.name);
            if x >= width {
                break;
            }
            if x + span <= 0.0 {
                x += span;
                continue;
            }
            let left = x.max(0.0);
            let rect = Rect::new(left, 0.0, (x + span).min(width) - left, TAB_BAR_HEIGHT);
            let active = index == model.active_tab;
            self.push_quad(
                rect,
                if active { TAB_ACTIVE_FILL } else { TAB_FILL },
                LAYER_BAR,
            );
            self.push_quad(
                Rect::new(
                    rect.right() - EDGE_THICKNESS,
                    0.0,
                    EDGE_THICKNESS,
                    TAB_BAR_HEIGHT,
                ),
                BAR_EDGE,
                LAYER_BAR,
            );
            if active {
                self.push_quad(
                    Rect::new(rect.x, TAB_BAR_HEIGHT - 0.16, rect.w, 0.16),
                    TAB_ACCENT,
                    LAYER_BAR_TEXT,
                );
            }
            let baseline = baseline_of(rect);
            let color = if active { TEXT_MAIN } else { TEXT_DIM };
            let shown = tab.name.chars().count().min(TAB_NAME_MAX);
            self.push_clipped(
                x + TAB_PAD,
                baseline,
                color,
                LAYER_BAR_TEXT,
                &tab.name,
                TAB_NAME_MAX,
            );
            if tab.modified {
                let dot = x + TAB_PAD + shown as f32 * CHAR_WIDTH + TAB_GAP + 0.14;
                if dot >= 0.0 && dot < width {
                    self.push_quad(
                        Rect::new(dot, baseline - 0.5, 0.38, 0.38),
                        MODIFIED_COLOR,
                        LAYER_BAR_TEXT,
                    );
                }
            }
            self.push_hit(rect, Target::Tab(index));
            let close = Rect::new(
                x + span - TAB_PAD - TAB_CLOSE_WIDTH,
                0.0,
                TAB_CLOSE_WIDTH,
                TAB_BAR_HEIGHT,
            );
            if close.x >= 0.0 && close.right() <= width {
                self.push_label(
                    close.x + (TAB_CLOSE_WIDTH - CHAR_WIDTH) * 0.5,
                    baseline,
                    if active { TEXT_MAIN } else { TEXT_FAINT },
                    LAYER_BAR_TEXT,
                    "x",
                );
                self.push_hit(close, Target::TabClose(index));
            }
            x += span;
        }
    }

    fn build_tree(&mut self, width: f32, top: f32, bottom: f32, model: &HudModel) {
        let area = Rect::new(0.0, top, width, (bottom - top).max(0.0));
        self.push_quad(area, SIDEBAR_FILL, LAYER_BAR);
        self.push_quad(
            Rect::new(width - EDGE_THICKNESS, top, EDGE_THICKNESS, area.h),
            BAR_EDGE,
            LAYER_BAR,
        );
        self.push_region(area, Region::Tree);

        let entries = model.project.entries();
        let visible = (area.h / TREE_ROW_HEIGHT).floor().max(0.0) as usize;
        if visible == 0 || entries.is_empty() {
            return;
        }
        let max_scroll = entries.len().saturating_sub(visible);
        let scroll = model.overlay.tree_scroll().min(max_scroll);
        let selection = model.overlay.sidebar_selection();
        let bar = if entries.len() > visible {
            SCROLLBAR_WIDTH
        } else {
            0.0
        };

        for slot in 0..visible {
            let index = scroll + slot;
            let Some(entry) = entries.get(index) else {
                break;
            };
            let row = Rect::new(
                0.0,
                top + slot as f32 * TREE_ROW_HEIGHT,
                width,
                TREE_ROW_HEIGHT,
            );
            if index == selection {
                self.push_quad(row, SELECTED_ROW, LAYER_BAR);
            }
            let baseline = baseline_of(row);
            let indent = TREE_PAD + entry.depth as f32 * TREE_INDENT;
            if entry.is_dir {
                let toggle = Rect::new(indent, row.y, CHEVRON_WIDTH, TREE_ROW_HEIGHT);
                self.push_label(
                    toggle.x + 0.28,
                    baseline,
                    TEXT_DIM,
                    LAYER_BAR_TEXT,
                    if entry.expanded { "v" } else { ">" },
                );
                self.push_hit(row, Target::TreeRow(index));
                self.push_hit(toggle, Target::TreeToggle(index));
            } else {
                self.push_hit(row, Target::TreeRow(index));
            }
            let name_x = indent + CHEVRON_WIDTH;
            let room = width - name_x - TREE_PAD - bar;
            let max_chars = (room / CHAR_WIDTH).floor().max(0.0) as usize;
            let color = if entry.is_dir {
                DIR_COLOR
            } else if index == selection {
                TEXT_MAIN
            } else {
                TEXT_DIM
            };
            self.push_clipped(
                name_x,
                baseline,
                color,
                LAYER_BAR_TEXT,
                &entry.name,
                max_chars,
            );
        }

        if bar > 0.0 {
            let track = Rect::new(width - bar, top, bar, area.h);
            self.push_scrollbar(track, entries.len(), visible, scroll);
            self.push_hit(track, Target::Scrollbar(Region::Tree));
        }
    }

    fn build_output(&mut self, width: f32, top: f32, bottom: f32, model: &HudModel) {
        let area = Rect::new(0.0, top, width, (bottom - top).max(0.0));
        self.push_quad(area, BAR_FILL, LAYER_BAR);
        self.push_quad(
            Rect::new(0.0, top, width, EDGE_THICKNESS),
            BAR_EDGE,
            LAYER_BAR,
        );
        self.push_region(area, Region::Output);

        let header = Rect::new(0.0, top, width, OUTPUT_HEADER_HEIGHT.min(area.h));
        let baseline = baseline_of(header);
        let mut x = STATUS_PAD;
        x += self.push_label(x, baseline, TEXT_DIM, LAYER_BAR_TEXT, "sortie") + STATUS_GAP;
        if !model.tasks.label().is_empty() {
            x += self.push_label(
                x,
                baseline,
                TEXT_ACCENT,
                LAYER_BAR_TEXT,
                model.tasks.label(),
            ) + STATUS_GAP;
        }
        self.scratch.clear();
        if model.tasks.running() {
            self.scratch.push_str("en cours");
        } else {
            match model.tasks.exit_code() {
                Some(0) => self.scratch.push_str("termine"),
                Some(code) => {
                    let _ = write!(self.scratch, "echec {}", code);
                }
                None => self.scratch.push_str("inactif"),
            }
        }
        let color = if model.tasks.running() {
            RUNNING_COLOR
        } else {
            match model.tasks.exit_code() {
                Some(0) => RUNNING_COLOR,
                Some(_) => ERROR_COLOR,
                None => TEXT_FAINT,
            }
        };
        self.emit_scratch(x, baseline, color, LAYER_BAR_TEXT);

        let list_top = top + header.h;
        let list_height = (bottom - list_top).max(0.0);
        let visible = (list_height / OUTPUT_ROW_HEIGHT).floor().max(0.0) as usize;
        let lines = model.tasks.lines();
        if visible == 0 || lines.is_empty() {
            return;
        }
        let first = output_window(lines.len(), visible, model.overlay.output_scroll());
        let bar = if lines.len() > visible {
            SCROLLBAR_WIDTH
        } else {
            0.0
        };
        let room = width - STATUS_PAD * 2.0 - bar;
        let max_chars = (room / CHAR_WIDTH).floor().max(0.0) as usize;

        for slot in 0..visible {
            let index = first + slot;
            let Some(line) = lines.get(index) else {
                break;
            };
            let row = Rect::new(
                0.0,
                list_top + slot as f32 * OUTPUT_ROW_HEIGHT,
                width,
                OUTPUT_ROW_HEIGHT,
            );
            let color = match line.stream {
                Stream::Err => STDERR_COLOR,
                Stream::Out => STDOUT_COLOR,
            };
            self.push_clipped(
                STATUS_PAD,
                baseline_of(row),
                color,
                LAYER_BAR_TEXT,
                &line.text,
                max_chars,
            );
            self.push_hit(row, Target::OutputLine(index));
        }

        if bar > 0.0 {
            let track = Rect::new(width - bar, list_top, bar, list_height);
            self.push_scrollbar(track, lines.len(), visible, first);
            self.push_hit(track, Target::Scrollbar(Region::Output));
        }
    }

    fn build_status(&mut self, width: f32, top: f32, model: &HudModel) {
        let bar = Rect::new(0.0, top, width, STATUS_HEIGHT);
        self.push_quad(bar, BAR_FILL, LAYER_BAR);
        self.push_quad(
            Rect::new(0.0, top, width, EDGE_THICKNESS),
            BAR_EDGE,
            LAYER_BAR,
        );
        let baseline = baseline_of(bar);

        let mut right = width - STATUS_PAD;
        let font_width = model.font_name.chars().count() as f32 * CHAR_WIDTH;
        if font_width > 0.0 && font_width < right {
            self.push_label(
                right - font_width,
                baseline,
                TEXT_FAINT,
                LAYER_BAR_TEXT,
                model.font_name,
            );
            right -= font_width + STATUS_GAP;
        }
        self.scratch.clear();
        let _ = write!(
            self.scratch,
            "ligne {}, col {}",
            model.cursor.line + 1,
            model.cursor.column + 1
        );
        let cursor_width = self.scratch.chars().count() as f32 * CHAR_WIDTH;
        if cursor_width < right {
            self.emit_scratch(right - cursor_width, baseline, TEXT_DIM, LAYER_BAR_TEXT);
            right -= cursor_width + STATUS_GAP;
        }

        let limit = right;
        let mut x = STATUS_PAD;

        if !fits(x, model.project.label(), limit) {
            return;
        }
        x += self.push_label(
            x,
            baseline,
            TEXT_ACCENT,
            LAYER_BAR_TEXT,
            model.project.label(),
        ) + STATUS_GAP;

        let language = match model.language {
            Some(language) => language.label(),
            None => "texte",
        };
        let kind = model.project.kind().label();
        if kind != language {
            if !fits(x, kind, limit) {
                return;
            }
            x += self.push_label(x, baseline, TEXT_FAINT, LAYER_BAR_TEXT, kind) + STATUS_GAP;
        }

        if !fits(x, language, limit) {
            return;
        }
        x += self.push_label(x, baseline, TEXT_DIM, LAYER_BAR_TEXT, language) + STATUS_GAP;

        if !fits(x, model.server_status, limit) {
            return;
        }
        let server_width =
            self.push_label(x, baseline, TEXT_DIM, LAYER_BAR_TEXT, model.server_status);
        self.push_hit(
            Rect::new(
                x - STATUS_GAP * 0.5,
                top,
                server_width + STATUS_GAP,
                STATUS_HEIGHT,
            ),
            Target::StatusServer,
        );
        x += server_width + STATUS_GAP;

        let (errors, warnings) = model.diagnostics;
        self.scratch.clear();
        let _ = write!(
            self.scratch,
            "{} erreurs  {} avertissements",
            errors, warnings
        );
        if !fits_scratch(x, &self.scratch, limit) {
            return;
        }
        let color = if errors > 0 {
            ERROR_COLOR
        } else if warnings > 0 {
            WARNING_COLOR
        } else {
            TEXT_FAINT
        };
        let diagnostics_width = self.emit_scratch(x, baseline, color, LAYER_BAR_TEXT);
        self.push_hit(
            Rect::new(
                x - STATUS_GAP * 0.5,
                top,
                diagnostics_width + STATUS_GAP,
                STATUS_HEIGHT,
            ),
            Target::StatusDiagnostics,
        );
        x += diagnostics_width + STATUS_GAP;

        if model.tasks.running() {
            self.scratch.clear();
            let _ = write!(self.scratch, "tache: {}", model.tasks.label());
            if !fits_scratch(x, &self.scratch, limit) {
                return;
            }
            x += self.emit_scratch(x, baseline, RUNNING_COLOR, LAYER_BAR_TEXT) + STATUS_GAP;
        } else if let Some(code) = model.tasks.exit_code() {
            self.scratch.clear();
            if code == 0 {
                let _ = write!(self.scratch, "{}: ok", model.tasks.label());
            } else {
                let _ = write!(self.scratch, "{}: echec {}", model.tasks.label(), code);
            }
            if !fits_scratch(x, &self.scratch, limit) {
                return;
            }
            let color = if code == 0 {
                RUNNING_COLOR
            } else {
                ERROR_COLOR
            };
            x += self.emit_scratch(x, baseline, color, LAYER_BAR_TEXT) + STATUS_GAP;
        }

        if model.search.running() {
            let (done, total) = model.search.progress();
            self.scratch.clear();
            let _ = write!(self.scratch, "recherche {}/{}", done, total);
            if !fits_scratch(x, &self.scratch, limit) {
                return;
            }
            let _ = self.emit_scratch(x, baseline, TEXT_DIM, LAYER_BAR_TEXT);
        }
    }

    fn build_palette(&mut self, width: f32, height: f32, model: &HudModel) {
        let overlay = model.overlay;
        let panel_width = PALETTE_WIDTH.min((width - 8.0).max(16.0));
        let x = ((width - panel_width) * 0.5).max(0.0);
        let rows = overlay.visible_rows();
        let body = rows.len() as f32 * PANEL_ROW_HEIGHT;
        let total = PANEL_PAD * 2.0 + PANEL_ROW_HEIGHT * 2.0 + EDGE_THICKNESS + body;
        let y = PALETTE_TOP.min((height - total - 1.0).max(0.0));
        let panel = Rect::new(x, y, panel_width, total);

        self.push_quad(
            Rect::new(
                x - PANEL_EDGE_WIDTH,
                y - PANEL_EDGE_WIDTH,
                panel_width + PANEL_EDGE_WIDTH * 2.0,
                total + PANEL_EDGE_WIDTH * 2.0,
            ),
            PANEL_EDGE,
            LAYER_PANEL,
        );
        self.push_quad(panel, PANEL_FILL, LAYER_PANEL);
        self.push_region(panel, Region::Panel);
        self.push_hit(panel, Target::None);

        let input = Rect::new(x, y + PANEL_PAD, panel_width, PANEL_ROW_HEIGHT);
        let baseline = baseline_of(input);
        let mut cursor_x = x + PANEL_PAD;
        cursor_x += self.push_label(
            cursor_x,
            baseline,
            TEXT_ACCENT,
            LAYER_PANEL_TEXT,
            overlay.title(),
        ) + CHAR_WIDTH;
        cursor_x +=
            self.push_label(cursor_x, baseline, TEXT_FAINT, LAYER_PANEL_TEXT, ">") + CHAR_WIDTH;
        let room = panel_width - PANEL_PAD - (cursor_x - x);
        let max_query = (room / CHAR_WIDTH).floor().max(0.0) as usize;
        let query = overlay.query();
        let shown = query.chars().count().min(max_query);
        let tail = skip_chars(query, query.chars().count() - shown);
        cursor_x += self.push_label(cursor_x, baseline, TEXT_MAIN, LAYER_PANEL_TEXT, tail);
        self.push_quad(
            Rect::new(
                cursor_x + 0.04,
                baseline - CAP_HEIGHT - 0.14,
                CARET_WIDTH,
                CAP_HEIGHT + 0.3,
            ),
            CARET_COLOR,
            LAYER_PANEL_TEXT,
        );
        let separator = input.bottom();
        self.push_quad(
            Rect::new(x, separator, panel_width, EDGE_THICKNESS),
            PANEL_EDGE,
            LAYER_PANEL,
        );

        let list_top = separator + EDGE_THICKNESS;
        let total_rows = overlay.rows().len();
        let bar = if total_rows > rows.len() {
            SCROLLBAR_WIDTH
        } else {
            0.0
        };
        let selected = overlay.selected().map(|row| row as *const Row);
        let mut position = None;
        for (slot, row) in rows.iter().enumerate() {
            let rect = Rect::new(
                x,
                list_top + slot as f32 * PANEL_ROW_HEIGHT,
                panel_width,
                PANEL_ROW_HEIGHT,
            );
            let current = selected == Some(row as *const Row);
            if current {
                self.push_quad(rect, PANEL_SELECTED, LAYER_PANEL);
                position = Some(overlay.scroll() + slot);
            }
            let line = baseline_of(rect);
            let mut left = x + PANEL_PAD;
            if !row.tag.is_empty() {
                left +=
                    self.push_label(left, line, TEXT_FAINT, LAYER_PANEL_TEXT, row.tag) + CHAR_WIDTH;
            }
            let detail_room = (panel_width * DETAIL_FRACTION - PANEL_PAD).max(0.0);
            let detail_max = (detail_room / CHAR_WIDTH).floor().max(0.0) as usize;
            let detail_shown = row.detail.chars().count().min(detail_max);
            let detail_width = detail_shown as f32 * CHAR_WIDTH;
            let detail_x = x + panel_width - PANEL_PAD - bar - detail_width;
            let label_room = (detail_x - CHAR_WIDTH - left).max(0.0);
            let label_max = (label_room / CHAR_WIDTH).floor().max(0.0) as usize;
            self.push_clipped(
                left,
                line,
                if current { TEXT_MAIN } else { TEXT_DIM },
                LAYER_PANEL_TEXT,
                &row.label,
                label_max,
            );
            if detail_shown > 0 {
                self.push_clipped(
                    detail_x,
                    line,
                    TEXT_FAINT,
                    LAYER_PANEL_TEXT,
                    &row.detail,
                    detail_max,
                );
            }
            self.push_hit(rect, Target::PanelRow(slot));
        }

        if bar > 0.0 {
            let track = Rect::new(
                x + panel_width - bar,
                list_top,
                bar,
                rows.len() as f32 * PANEL_ROW_HEIGHT,
            );
            self.push_scrollbar(track, total_rows, rows.len(), overlay.scroll());
            self.push_hit(track, Target::Scrollbar(Region::Panel));
        }

        let footer = Rect::new(
            x,
            list_top + rows.len() as f32 * PANEL_ROW_HEIGHT,
            panel_width,
            PANEL_ROW_HEIGHT,
        );
        let line = baseline_of(footer);
        if !overlay.status().is_empty() {
            let room = panel_width - PANEL_PAD * 2.0;
            let max_chars = (room / CHAR_WIDTH).floor().max(0.0) as usize;
            self.push_clipped(
                x + PANEL_PAD,
                line,
                TEXT_FAINT,
                LAYER_PANEL_TEXT,
                overlay.status(),
                max_chars,
            );
        }
        self.scratch.clear();
        match position {
            Some(index) => {
                let _ = write!(self.scratch, "{} sur {}", index + 1, total_rows);
            }
            None => {
                let _ = write!(self.scratch, "{} lignes", total_rows);
            }
        }
        let counter = self.scratch.chars().count() as f32 * CHAR_WIDTH;
        self.emit_scratch(
            x + panel_width - PANEL_PAD - counter,
            line,
            TEXT_FAINT,
            LAYER_PANEL_TEXT,
        );
    }

    fn build_cards(&mut self, model: &HudModel) {
        if self.editor.w <= CARD_MIN_WIDTH || self.editor.h <= CARD_ROW_HEIGHT * 2.0 {
            return;
        }
        let anchor_x = self.editor.x + self.editor.w * 0.5;
        let anchor_y = self.editor.y + self.editor.h * 0.5;
        if let Some(signature) = model.signature {
            self.build_signature_card(signature, anchor_x, anchor_y);
        }
        if let Some(hover) = model.hover.filter(|text| !text.is_empty()) {
            self.build_hover_card(hover, anchor_x, anchor_y);
        }
    }

    fn build_hover_card(&mut self, text: &str, anchor_x: f32, anchor_y: f32) {
        let max_chars = self.card_max_chars();
        wrap_text(text, max_chars, &mut self.wrap);
        let lines = self.wrap.len().min(CARD_MAX_LINES);
        if lines == 0 {
            return;
        }
        let widest = widest_line(text, &self.wrap[..lines]);
        let card = self.place_card(widest, lines, anchor_x, anchor_y, false);
        self.push_card_frame(card);
        let mut baseline = card.y + CARD_PAD + CAP_HEIGHT;
        for slot in 0..lines {
            let (start, end) = self.wrap[slot];
            let piece = &text[start..end];
            self.push_label(
                card.x + CARD_PAD,
                baseline,
                TEXT_MAIN,
                LAYER_CARD_TEXT,
                piece,
            );
            baseline += CARD_ROW_HEIGHT;
        }
        self.push_hit(card, Target::None);
    }

    fn build_signature_card(&mut self, signature: &SignatureInfo, anchor_x: f32, anchor_y: f32) {
        let max_chars = self.card_max_chars();
        if max_chars == 0 {
            return;
        }
        let active = signature
            .parameters
            .get(signature.active_parameter)
            .map(String::as_str)
            .unwrap_or("");
        let label_chars = signature.label.chars().count().min(max_chars);
        let mut lines = 1usize;
        let mut widest = label_chars;
        if !signature.documentation.is_empty() {
            wrap_text(&signature.documentation, max_chars, &mut self.wrap);
            let extra = self.wrap.len().min(CARD_MAX_LINES - 1);
            widest = widest.max(widest_line(&signature.documentation, &self.wrap[..extra]));
            lines += extra;
        } else {
            self.wrap.clear();
        }
        let card = self.place_card(widest, lines, anchor_x, anchor_y, true);
        self.push_card_frame(card);
        let mut baseline = card.y + CARD_PAD + CAP_HEIGHT;
        let split = if signature.label.chars().count() <= max_chars && !active.is_empty() {
            signature.label.find(active)
        } else {
            None
        };
        match split {
            Some(offset) => {
                let head = &signature.label[..offset];
                let tail = &signature.label[offset + active.len()..];
                let mut x = card.x + CARD_PAD;
                x += self.push_label(x, baseline, TEXT_DIM, LAYER_CARD_TEXT, head);
                x += self.push_label(x, baseline, TEXT_ACCENT, LAYER_CARD_TEXT, active);
                self.push_label(x, baseline, TEXT_DIM, LAYER_CARD_TEXT, tail);
            }
            None => {
                self.push_clipped(
                    card.x + CARD_PAD,
                    baseline,
                    TEXT_MAIN,
                    LAYER_CARD_TEXT,
                    &signature.label,
                    max_chars,
                );
            }
        }
        baseline += CARD_ROW_HEIGHT;
        for slot in 0..lines - 1 {
            let (start, end) = self.wrap[slot];
            let piece = &signature.documentation[start..end];
            self.push_label(
                card.x + CARD_PAD,
                baseline,
                TEXT_FAINT,
                LAYER_CARD_TEXT,
                piece,
            );
            baseline += CARD_ROW_HEIGHT;
        }
        self.push_hit(card, Target::None);
    }

    fn card_max_chars(&self) -> usize {
        let room = CARD_MAX_WIDTH.min(self.editor.w - 2.0) - CARD_PAD * 2.0;
        (room / CHAR_WIDTH).floor().max(0.0) as usize
    }

    fn place_card(
        &self,
        widest: usize,
        lines: usize,
        anchor_x: f32,
        anchor_y: f32,
        above: bool,
    ) -> Rect {
        let width = (widest as f32 * CHAR_WIDTH + CARD_PAD * 2.0)
            .max(CARD_MIN_WIDTH)
            .min(self.editor.w - 1.0);
        let height = lines as f32 * CARD_ROW_HEIGHT + CARD_PAD * 2.0;
        let mut y = if above {
            anchor_y - CARD_GAP - height
        } else {
            anchor_y + CARD_GAP
        };
        if y < self.editor.y {
            y = anchor_y + CARD_GAP;
        }
        if y + height > self.editor.bottom() {
            y = (anchor_y - CARD_GAP - height).max(self.editor.y);
        }
        let x = (anchor_x - width * 0.5)
            .max(self.editor.x + 0.5)
            .min(self.editor.right() - width - 0.5);
        Rect::new(x, y, width, height)
    }

    fn push_card_frame(&mut self, card: Rect) {
        self.push_quad(
            Rect::new(
                card.x - PANEL_EDGE_WIDTH,
                card.y - PANEL_EDGE_WIDTH,
                card.w + PANEL_EDGE_WIDTH * 2.0,
                card.h + PANEL_EDGE_WIDTH * 2.0,
            ),
            CARD_EDGE,
            LAYER_CARD,
        );
        self.push_quad(card, CARD_FILL, LAYER_CARD);
    }

    fn push_scrollbar(&mut self, track: Rect, total: usize, visible: usize, first: usize) {
        self.push_quad(track, SCROLLBAR_TRACK, LAYER_BAR);
        if total == 0 || visible == 0 {
            return;
        }
        let ratio = (visible as f32 / total as f32).min(1.0);
        let thumb_height = (track.h * ratio).max(0.8).min(track.h);
        let span = (track.h - thumb_height).max(0.0);
        let max_first = total.saturating_sub(visible).max(1) as f32;
        let offset = span * (first as f32 / max_first).min(1.0);
        self.push_quad(
            Rect::new(track.x, track.y + offset, track.w, thumb_height),
            SCROLLBAR_THUMB,
            LAYER_BAR_TEXT,
        );
    }

    fn push_quad(&mut self, rect: Rect, color: [u8; 4], layer: u8) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let quad = Quad { rect, color, layer };
        if self.quad_count == self.quads.len() {
            self.quads.push(quad);
        } else {
            self.quads[self.quad_count] = quad;
        }
        self.quad_count += 1;
    }

    fn push_hit(&mut self, rect: Rect, target: Target) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        if self.hit_count == self.hits.len() {
            self.hits.push((rect, target));
        } else {
            self.hits[self.hit_count] = (rect, target);
        }
        self.hit_count += 1;
    }

    fn push_region(&mut self, rect: Rect, region: Region) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        if self.region_count == self.regions.len() {
            self.regions.push((rect, region));
        } else {
            self.regions[self.region_count] = (rect, region);
        }
        self.region_count += 1;
    }

    fn reserve_label(&mut self, x: f32, y: f32, color: [u8; 4], layer: u8) -> &mut String {
        if self.label_count == self.labels.len() {
            self.labels.push(Label {
                x,
                y,
                text: String::with_capacity(48),
                color,
                layer,
            });
        } else {
            let slot = &mut self.labels[self.label_count];
            slot.x = x;
            slot.y = y;
            slot.color = color;
            slot.layer = layer;
            slot.text.clear();
        }
        let index = self.label_count;
        self.label_count += 1;
        &mut self.labels[index].text
    }

    fn push_label(&mut self, x: f32, y: f32, color: [u8; 4], layer: u8, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let width = text.chars().count() as f32 * CHAR_WIDTH;
        self.reserve_label(x, y, color, layer).push_str(text);
        width
    }

    fn push_clipped(
        &mut self,
        x: f32,
        y: f32,
        color: [u8; 4],
        layer: u8,
        text: &str,
        max_chars: usize,
    ) -> f32 {
        if max_chars == 0 || text.is_empty() {
            return 0.0;
        }
        let count = text.chars().count();
        if count <= max_chars {
            return self.push_label(x, y, color, layer, text);
        }
        let slot = self.reserve_label(x, y, color, layer);
        if max_chars <= ELLIPSIS_CHARS {
            for ch in text.chars().take(max_chars) {
                slot.push(ch);
            }
        } else {
            for ch in text.chars().take(max_chars - ELLIPSIS_CHARS) {
                slot.push(ch);
            }
            slot.push_str(ELLIPSIS);
        }
        max_chars as f32 * CHAR_WIDTH
    }

    fn emit_scratch(&mut self, x: f32, y: f32, color: [u8; 4], layer: u8) -> f32 {
        let text = std::mem::take(&mut self.scratch);
        let width = self.push_label(x, y, color, layer, &text);
        self.scratch = text;
        width
    }
}

fn output_window(total: usize, visible: usize, from_top: usize) -> usize {
    from_top.min(total.saturating_sub(visible))
}

fn tab_span(name: &str) -> f32 {
    let shown = name.chars().count().min(TAB_NAME_MAX) as f32;
    TAB_PAD + shown * CHAR_WIDTH + TAB_GAP + TAB_MODIFIED_WIDTH + TAB_CLOSE_WIDTH + TAB_PAD
}

fn fits(x: f32, text: &str, limit: f32) -> bool {
    x + text.chars().count() as f32 * CHAR_WIDTH <= limit
}

fn fits_scratch(x: f32, scratch: &str, limit: f32) -> bool {
    fits(x, scratch, limit)
}

fn baseline_of(rect: Rect) -> f32 {
    rect.y + (rect.h + CAP_HEIGHT) * 0.5
}

fn skip_chars(text: &str, count: usize) -> &str {
    if count == 0 {
        return text;
    }
    match text.char_indices().nth(count) {
        Some((offset, _)) => &text[offset..],
        None => "",
    }
}

fn widest_line(text: &str, ranges: &[(usize, usize)]) -> usize {
    let mut widest = 0usize;
    for &(start, end) in ranges {
        let count = text[start..end].chars().count();
        if count > widest {
            widest = count;
        }
    }
    widest
}

fn wrap_text(text: &str, max_chars: usize, out: &mut Vec<(usize, usize)>) {
    out.clear();
    if max_chars == 0 {
        return;
    }
    let mut line_start = 0usize;
    let mut count = 0usize;
    let mut last_space: Option<usize> = None;
    for (offset, ch) in text.char_indices() {
        let next = offset + ch.len_utf8();
        if ch == '\n' {
            out.push((line_start, offset));
            line_start = next;
            count = 0;
            last_space = None;
            continue;
        }
        if ch == ' ' || ch == '\t' {
            last_space = Some(offset);
        }
        count += 1;
        if count > max_chars {
            match last_space {
                Some(space) if space > line_start => {
                    out.push((line_start, space));
                    line_start = space + 1;
                    count = text[line_start..next].chars().count();
                }
                _ => {
                    out.push((line_start, offset));
                    line_start = offset;
                    count = 1;
                }
            }
            last_space = None;
        }
    }
    if line_start < text.len() || out.is_empty() {
        out.push((line_start, text.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{Row, RowKind};
    use crate::tasks::Task;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const VIEW_WIDTH: f32 = 120.0;
    const VIEW_HEIGHT: f32 = 60.0;

    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Sandbox {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "text3d-hud-{}-{}-{}",
                std::process::id(),
                name,
                unique
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("sous")).expect("dossier de test");
            fs::write(root.join("aaa.rs"), "fn main() {}\n").expect("fichier de test");
            fs::write(root.join("bbb.rs"), "fn autre() {}\n").expect("fichier de test");
            Sandbox { root }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct Fixture {
        sandbox: Sandbox,
        project: Project,
        tasks: TaskRunner,
        search: ProjectSearch,
        overlay: Overlay,
        tabs: Vec<Tab>,
        hover: Option<String>,
        signature: Option<SignatureInfo>,
        active: usize,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let sandbox = Sandbox::new(name);
            let project = Project::open(&sandbox.root);
            let tabs = vec![
                tab("aaa.rs", false),
                tab("bbb.rs", true),
                tab("un_nom_de_fichier_vraiment_tres_long.rs", false),
            ];
            Fixture {
                sandbox,
                project,
                tasks: TaskRunner::new(),
                search: ProjectSearch::new(),
                overlay: Overlay::new(),
                tabs,
                hover: None,
                signature: None,
                active: 0,
            }
        }

        fn model(&self) -> HudModel<'_> {
            HudModel {
                project: &self.project,
                tabs: &self.tabs,
                active_tab: self.active,
                overlay: &self.overlay,
                tasks: &self.tasks,
                search: &self.search,
                server_status: "serveur pret",
                diagnostics: (2, 3),
                cursor: Cursor { line: 4, column: 7 },
                language: Some(Language::Rust),
                hover: self.hover.as_deref(),
                signature: self.signature.as_ref(),
                font_name: "inter",
            }
        }

        fn build(&self, hud: &mut Hud) {
            hud.build(
                HudViewport {
                    width: VIEW_WIDTH,
                    height: VIEW_HEIGHT,
                },
                &self.model(),
            );
        }
    }

    fn tab(name: &str, modified: bool) -> Tab {
        Tab {
            path: Some(PathBuf::from(name)),
            name: name.to_string(),
            modified,
            language: Some(Language::Rust),
        }
    }

    fn sample_row(label: &str, detail: &str) -> Row {
        Row {
            label: label.to_string(),
            detail: detail.to_string(),
            tag: "",
            kind: RowKind::File,
            target: None,
        }
    }

    fn tab_width(name: &str) -> f32 {
        tab_span(name)
    }

    #[test]
    fn tab_borders_are_exact() {
        let fixture = Fixture::new("onglets");
        let mut hud = Hud::new();
        fixture.build(&mut hud);

        let first = tab_width("aaa.rs");
        let second = tab_width("bbb.rs");
        assert_eq!(hud.hit(0.0, 0.0), Target::Tab(0));
        assert_eq!(
            hud.hit(first - 0.001, TAB_BAR_HEIGHT - 0.001),
            Target::Tab(0)
        );
        assert_eq!(hud.hit(first, 0.4), Target::Tab(1));
        assert_eq!(hud.hit(first + second - 0.001, 0.4), Target::Tab(1));
        assert_eq!(hud.hit(first + second, 0.4), Target::Tab(2));
        assert_ne!(hud.hit(0.2, TAB_BAR_HEIGHT), Target::Tab(0));
    }

    #[test]
    fn tab_close_zone_borders_are_exact() {
        let fixture = Fixture::new("fermeture");
        let mut hud = Hud::new();
        fixture.build(&mut hud);

        let first = tab_width("aaa.rs");
        let close_start = first - TAB_PAD - TAB_CLOSE_WIDTH;
        let close_end = close_start + TAB_CLOSE_WIDTH;
        assert_eq!(hud.hit(close_start - 0.001, 0.9), Target::Tab(0));
        assert_eq!(hud.hit(close_start, 0.9), Target::TabClose(0));
        assert_eq!(hud.hit(close_end - 0.001, 0.9), Target::TabClose(0));
        assert_eq!(hud.hit(close_end, 0.9), Target::Tab(0));
        assert_eq!(hud.hit(close_start, 0.0), Target::TabClose(0));
        assert_eq!(
            hud.hit(close_start, TAB_BAR_HEIGHT - 0.001),
            Target::TabClose(0)
        );
        assert_ne!(hud.hit(close_start, TAB_BAR_HEIGHT), Target::TabClose(0));

        let second_close = first + tab_width("bbb.rs") - TAB_PAD - TAB_CLOSE_WIDTH;
        assert_eq!(hud.hit(second_close, 0.9), Target::TabClose(1));
    }

    #[test]
    fn tab_strip_scrolls_to_keep_the_active_tab_visible() {
        let mut fixture = Fixture::new("defilement-onglets");
        fixture.tabs = (0..30)
            .map(|index| tab(&format!("fichier{:02}.rs", index), false))
            .collect();
        fixture.active = 29;
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let span = tab_span("fichier29.rs");
        let slack = 0.01;
        assert_eq!(hud.hit(VIEW_WIDTH - 0.001, 0.9), Target::Tab(29));
        assert_eq!(hud.hit(VIEW_WIDTH - span + slack, 0.9), Target::Tab(29));
        assert_eq!(hud.hit(VIEW_WIDTH - span - slack, 0.9), Target::Tab(28));
        assert_eq!(
            hud.hit(VIEW_WIDTH - TAB_PAD - TAB_CLOSE_WIDTH + slack, 0.9),
            Target::TabClose(29)
        );
        assert_eq!(
            hud.hit(VIEW_WIDTH - TAB_PAD - TAB_CLOSE_WIDTH - slack, 0.9),
            Target::Tab(29)
        );
        fixture.active = 0;
        fixture.build(&mut hud);
        assert_eq!(hud.hit(0.0, 0.9), Target::Tab(0));
    }

    #[test]
    fn tree_rows_and_toggles_are_exact() {
        let fixture = Fixture::new("arbre");
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert!(fixture.project.entries().len() >= 4);
        assert!(fixture.project.entries()[0].is_dir);
        assert!(fixture.project.entries()[1].is_dir);

        let top = TAB_BAR_HEIGHT;
        assert_eq!(hud.hit(0.0, top), Target::TreeRow(0));
        assert_eq!(hud.hit(TREE_PAD - 0.001, top), Target::TreeRow(0));
        assert_eq!(hud.hit(TREE_PAD, top), Target::TreeToggle(0));
        assert_eq!(
            hud.hit(TREE_PAD + CHEVRON_WIDTH - 0.001, top),
            Target::TreeToggle(0)
        );
        assert_eq!(hud.hit(TREE_PAD + CHEVRON_WIDTH, top), Target::TreeRow(0));
        assert_eq!(
            hud.hit(TREE_PAD, top + TREE_ROW_HEIGHT - 0.001),
            Target::TreeToggle(0)
        );
        assert_eq!(hud.hit(TREE_PAD, top + TREE_ROW_HEIGHT), Target::TreeRow(1));

        let nested = TREE_PAD + TREE_INDENT;
        assert_eq!(
            hud.hit(nested - 0.001, top + TREE_ROW_HEIGHT),
            Target::TreeRow(1)
        );
        assert_eq!(
            hud.hit(nested, top + TREE_ROW_HEIGHT),
            Target::TreeToggle(1)
        );
        assert_eq!(
            hud.hit(nested, top + TREE_ROW_HEIGHT * 2.0),
            Target::TreeRow(2)
        );
        assert_eq!(hud.hit(SIDEBAR_WIDTH - 0.001, top), Target::TreeRow(0));
        assert_eq!(hud.hit(SIDEBAR_WIDTH, top), Target::Editor);
    }

    #[test]
    fn tree_scroll_offsets_row_indices() {
        let mut fixture = Fixture::new("defilement-arbre");
        for index in 0..80 {
            fs::write(
                fixture.sandbox.root.join(format!("f{:03}.rs", index)),
                "fn f() {}\n",
            )
            .expect("fichier de test");
        }
        fixture.project = Project::open(&fixture.sandbox.root);
        let total = fixture.project.entries().len();
        fixture.overlay.scroll_tree(3, total);
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert_eq!(fixture.overlay.tree_scroll(), 3);
        assert_eq!(
            hud.hit(SIDEBAR_WIDTH * 0.5, TAB_BAR_HEIGHT),
            Target::TreeRow(3)
        );
        assert_eq!(
            hud.hit(SIDEBAR_WIDTH * 0.5, TAB_BAR_HEIGHT + TREE_ROW_HEIGHT),
            Target::TreeRow(4)
        );
    }

    #[test]
    fn editor_rect_takes_the_remaining_space() {
        let mut fixture = Fixture::new("zone-editeur");
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let editor = hud.editor_rect();
        assert_eq!(editor.x, SIDEBAR_WIDTH);
        assert_eq!(editor.y, TAB_BAR_HEIGHT);
        assert_eq!(editor.w, VIEW_WIDTH - SIDEBAR_WIDTH);
        assert_eq!(editor.h, VIEW_HEIGHT - STATUS_HEIGHT - TAB_BAR_HEIGHT);
        assert_eq!(hud.hit(editor.x, editor.y), Target::Editor);
        assert_eq!(
            hud.hit(editor.right() - 0.001, editor.bottom() - 0.001),
            Target::Editor
        );
        assert_ne!(hud.hit(editor.x, editor.bottom()), Target::Editor);

        fixture.overlay.toggle_sidebar();
        fixture.overlay.toggle_output();
        fixture.build(&mut hud);
        let editor = hud.editor_rect();
        assert_eq!(editor.x, 0.0);
        assert_eq!(editor.w, VIEW_WIDTH);
        assert_eq!(
            editor.h,
            VIEW_HEIGHT - STATUS_HEIGHT - OUTPUT_HEIGHT - TAB_BAR_HEIGHT
        );
        assert_eq!(hud.hit(1.0, TAB_BAR_HEIGHT), Target::Editor);
    }

    #[test]
    fn palette_rows_are_exact_and_capture_everything_below() {
        let mut fixture = Fixture::new("palette");
        fixture.overlay.open(Panel::QuickOpen);
        let rows: Vec<Row> = (0..6)
            .map(|index| sample_row(&format!("fichier{}.rs", index), "src"))
            .collect();
        fixture.overlay.set_rows(rows);
        let mut hud = Hud::new();
        fixture.build(&mut hud);

        let panel_width = PALETTE_WIDTH.min(VIEW_WIDTH - 8.0);
        let panel_x = (VIEW_WIDTH - panel_width) * 0.5;
        let list_top = PALETTE_TOP + PANEL_PAD + PANEL_ROW_HEIGHT + EDGE_THICKNESS;
        let middle = panel_x + panel_width * 0.5;

        assert_eq!(hud.hit(middle, list_top), Target::PanelRow(0));
        assert_eq!(
            hud.hit(middle, list_top + PANEL_ROW_HEIGHT - 0.001),
            Target::PanelRow(0)
        );
        assert_eq!(
            hud.hit(middle, list_top + PANEL_ROW_HEIGHT),
            Target::PanelRow(1)
        );
        assert_eq!(
            hud.hit(middle, list_top + PANEL_ROW_HEIGHT * 5.0),
            Target::PanelRow(5)
        );
        assert_eq!(
            hud.hit(middle, list_top + PANEL_ROW_HEIGHT * 6.0),
            Target::None
        );
        assert_eq!(hud.hit(panel_x, list_top), Target::PanelRow(0));
        assert_eq!(hud.hit(panel_x - 0.001, list_top), Target::Editor);
        assert_eq!(
            hud.hit(panel_x + panel_width - 0.001, list_top),
            Target::PanelRow(0)
        );
        assert_eq!(hud.hit(panel_x + panel_width, list_top), Target::Editor);
        assert_eq!(hud.hit(middle, PALETTE_TOP), Target::None);
        assert_eq!(hud.region_at(middle, list_top), Some(Region::Panel));
    }

    #[test]
    fn palette_covers_the_tab_bar_when_it_reaches_it() {
        let mut fixture = Fixture::new("palette-couvre");
        fixture.overlay.open(Panel::Commands);
        let rows: Vec<Row> = (0..16)
            .map(|index| sample_row(&format!("commande {}", index), "cmd"))
            .collect();
        fixture.overlay.set_rows(rows);
        let mut hud = Hud::new();
        hud.build(
            HudViewport {
                width: VIEW_WIDTH,
                height: 30.0,
            },
            &fixture.model(),
        );
        let panel_width = PALETTE_WIDTH.min(VIEW_WIDTH - 8.0);
        let panel_x = (VIEW_WIDTH - panel_width) * 0.5;
        let panel = Rect::new(panel_x, 0.0, panel_width, 30.0);
        let mut covered = false;
        for step in 0..40 {
            let y = step as f32 * 0.5;
            if !panel.contains(panel_x + 1.0, y) {
                continue;
            }
            let target = hud.hit(panel_x + 1.0, y);
            if y < TAB_BAR_HEIGHT {
                assert!(
                    matches!(target, Target::None | Target::PanelRow(_)),
                    "la palette doit capturer la barre d onglets"
                );
                covered = true;
            }
        }
        assert!(covered);
    }

    #[test]
    fn palette_scrollbar_appears_when_rows_overflow() {
        let mut fixture = Fixture::new("palette-ascenseur");
        fixture.overlay.open(Panel::Problems);
        let rows: Vec<Row> = (0..40)
            .map(|index| sample_row(&format!("probleme {}", index), "src/main.rs"))
            .collect();
        fixture.overlay.set_rows(rows);
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert!(fixture.overlay.visible_rows().len() < fixture.overlay.rows().len());
        let panel_width = PALETTE_WIDTH.min(VIEW_WIDTH - 8.0);
        let panel_x = (VIEW_WIDTH - panel_width) * 0.5;
        let list_top = PALETTE_TOP + PANEL_PAD + PANEL_ROW_HEIGHT + EDGE_THICKNESS;
        assert_eq!(
            hud.hit(panel_x + panel_width - 0.001, list_top),
            Target::Scrollbar(Region::Panel)
        );
        assert_eq!(
            hud.hit(panel_x + panel_width - SCROLLBAR_WIDTH - 0.001, list_top),
            Target::PanelRow(0)
        );
    }

    #[test]
    fn palette_selection_follows_the_overlay() {
        let mut fixture = Fixture::new("palette-selection");
        fixture.overlay.open(Panel::WorkspaceSymbols);
        let rows: Vec<Row> = (0..4)
            .map(|index| sample_row(&format!("symbole{}", index), "detail"))
            .collect();
        fixture.overlay.set_rows(rows);
        fixture.overlay.move_selection(2);
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let counter = hud
            .labels()
            .iter()
            .find(|label| label.text.contains(" sur "))
            .expect("compteur de palette");
        assert_eq!(counter.text, "3 sur 4");
    }

    #[test]
    fn output_lines_are_exact() {
        let mut fixture = Fixture::new("sortie");
        fixture.overlay.toggle_output();
        fs::write(
            fixture.sandbox.root.join("Cargo.toml"),
            "[package]\nname = \"sondesortie\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"sondesortie\"\npath = \"aaa.rs\"\n",
        )
        .expect("manifeste de test");
        fixture.project = Project::open(&fixture.sandbox.root);
        let started = fixture.tasks.start(Task::Check, &fixture.project);
        if started.is_ok() {
            let deadline = Instant::now() + Duration::from_secs(90);
            while fixture.tasks.poll() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            fixture.tasks.poll();
        }
        assert!(
            !fixture.tasks.lines().is_empty(),
            "la tache doit produire des lignes"
        );

        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let output_top = VIEW_HEIGHT - STATUS_HEIGHT - OUTPUT_HEIGHT;
        let list_top = output_top + OUTPUT_HEADER_HEIGHT;
        let middle = VIEW_WIDTH * 0.5;
        assert_eq!(hud.hit(middle, list_top), Target::OutputLine(0));
        assert_eq!(
            hud.hit(middle, list_top + OUTPUT_ROW_HEIGHT - 0.001),
            Target::OutputLine(0)
        );
        assert_eq!(
            hud.hit(middle, list_top + OUTPUT_ROW_HEIGHT),
            Target::OutputLine(1)
        );
        assert_eq!(hud.hit(middle, list_top - 0.001), Target::None);
        assert_eq!(hud.hit(1.0, list_top), Target::OutputLine(0));
        assert_eq!(hud.region_at(middle, list_top), Some(Region::Output));
        assert_eq!(hud.region_at(middle, output_top - 0.001), None);
        assert_eq!(hud.region_at(1.0, output_top - 0.001), Some(Region::Tree));
    }

    #[test]
    fn output_window_anchors_at_the_bottom() {
        assert_eq!(output_window(0, 9, 0), 0);
        assert_eq!(output_window(4, 9, 0), 0);
        assert_eq!(output_window(40, 9, 31), 31);
        assert_eq!(output_window(40, 9, 26), 26);
        assert_eq!(output_window(40, 9, 999), 31);
    }

    #[test]
    fn status_zones_answer_the_pointer() {
        let fixture = Fixture::new("etat");
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let bar_y = VIEW_HEIGHT - STATUS_HEIGHT * 0.5;
        let mut server = false;
        let mut diagnostics = false;
        let mut x = 0.0;
        while x < VIEW_WIDTH {
            match hud.hit(x, bar_y) {
                Target::StatusServer => server = true,
                Target::StatusDiagnostics => diagnostics = true,
                _ => {}
            }
            x += 0.25;
        }
        assert!(server);
        assert!(diagnostics);
        let counts = hud
            .labels()
            .iter()
            .find(|label| label.text.contains("avertissements"))
            .expect("compteurs de diagnostics");
        assert_eq!(counts.text, "2 erreurs  3 avertissements");
        let position = hud
            .labels()
            .iter()
            .find(|label| label.text.starts_with("ligne "))
            .expect("position du curseur");
        assert_eq!(position.text, "ligne 5, col 8");
    }

    #[test]
    fn regions_drive_the_wheel() {
        let mut fixture = Fixture::new("molette");
        fixture.overlay.toggle_output();
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert_eq!(hud.region_at(1.0, TAB_BAR_HEIGHT), Some(Region::Tree));
        assert_eq!(hud.region_at(VIEW_WIDTH * 0.5, TAB_BAR_HEIGHT + 1.0), None);
        assert_eq!(
            hud.region_at(VIEW_WIDTH * 0.5, VIEW_HEIGHT - STATUS_HEIGHT - 1.0),
            Some(Region::Output)
        );
    }

    #[test]
    fn long_tab_names_are_truncated() {
        let fixture = Fixture::new("troncature");
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let label = hud
            .labels()
            .iter()
            .find(|label| label.text.starts_with("un_nom"))
            .expect("nom d onglet");
        assert_eq!(label.text.chars().count(), TAB_NAME_MAX);
        assert!(label.text.ends_with(ELLIPSIS));
    }

    #[test]
    fn hover_card_stays_inside_the_editor() {
        let mut fixture = Fixture::new("survol");
        fixture.hover = Some(
            "fn calculer(valeur: usize) -> usize\nrend la valeur doublee, avec une phrase assez longue pour forcer le repli sur plusieurs lignes de la carte"
                .to_string(),
        );
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let editor = hud.editor_rect();
        let card = hud
            .quads()
            .iter()
            .find(|quad| quad.layer == LAYER_CARD && quad.color == CARD_FILL)
            .expect("carte de survol");
        assert!(card.rect.x >= editor.x);
        assert!(card.rect.right() <= editor.right());
        assert!(card.rect.y >= editor.y);
        assert!(card.rect.bottom() <= editor.bottom());
        assert!(card.rect.w <= CARD_MAX_WIDTH);
        assert_eq!(hud.hit(card.rect.x, card.rect.y), Target::None);
        let lines = hud
            .labels()
            .iter()
            .filter(|label| label.layer == LAYER_CARD_TEXT)
            .count();
        assert!(lines >= 2);
    }

    #[test]
    fn signature_card_highlights_the_active_parameter() {
        let mut fixture = Fixture::new("signature");
        fixture.signature = Some(SignatureInfo {
            label: "fn joindre(gauche: &str, droite: &str) -> String".to_string(),
            documentation: "colle les deux morceaux".to_string(),
            parameters: vec!["gauche: &str".to_string(), "droite: &str".to_string()],
            active_parameter: 1,
        });
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        let active = hud
            .labels()
            .iter()
            .find(|label| label.color == TEXT_ACCENT && label.layer == LAYER_CARD_TEXT)
            .expect("parametre actif");
        assert_eq!(active.text, "droite: &str");
    }

    #[test]
    fn palette_hides_the_cards() {
        let mut fixture = Fixture::new("palette-sans-carte");
        fixture.hover = Some("une aide".to_string());
        fixture.overlay.open(Panel::Search);
        fixture.overlay.set_rows(vec![sample_row("resultat", "1")]);
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert!(hud.quads().iter().all(|quad| quad.color != CARD_FILL));
    }

    #[test]
    fn build_stops_allocating_after_the_first_frames() {
        let mut fixture = Fixture::new("perf");
        fixture.overlay.toggle_output();
        fixture.overlay.open(Panel::QuickOpen);
        fixture.overlay.set_rows(
            (0..12)
                .map(|index| sample_row(&format!("fichier{}.rs", index), "src/quelque/part"))
                .collect(),
        );
        fixture.hover = Some("une aide de survol".to_string());
        let mut hud = Hud::new();
        for _ in 0..4 {
            fixture.build(&mut hud);
        }
        let quads = hud.quads.capacity();
        let labels = hud.labels.capacity();
        let hits = hud.hits.capacity();
        let regions = hud.regions.capacity();
        let scratch = hud.scratch.capacity();
        let wrap = hud.wrap.capacity();
        let texts: Vec<usize> = hud
            .labels
            .iter()
            .map(|label| label.text.capacity())
            .collect();
        let quad_pointer = hud.quads.as_ptr();
        let label_pointer = hud.labels.as_ptr();
        for _ in 0..200 {
            fixture.build(&mut hud);
        }
        assert_eq!(hud.quads.capacity(), quads);
        assert_eq!(hud.labels.capacity(), labels);
        assert_eq!(hud.hits.capacity(), hits);
        assert_eq!(hud.regions.capacity(), regions);
        assert_eq!(hud.scratch.capacity(), scratch);
        assert_eq!(hud.wrap.capacity(), wrap);
        assert_eq!(hud.quads.as_ptr(), quad_pointer);
        assert_eq!(hud.labels.as_ptr(), label_pointer);
        let after: Vec<usize> = hud
            .labels
            .iter()
            .map(|label| label.text.capacity())
            .collect();
        assert_eq!(after, texts);
        assert!(!hud.quads().is_empty());
        assert!(!hud.labels().is_empty());
    }

    #[test]
    fn wrapping_respects_words_and_newlines() {
        let mut ranges = Vec::new();
        wrap_text("abc def ghi", 7, &mut ranges);
        let pieces: Vec<&str> = ranges
            .iter()
            .map(|&(start, end)| &"abc def ghi"[start..end])
            .collect();
        assert_eq!(pieces, vec!["abc def", "ghi"]);

        let text = "une\nligne";
        wrap_text(text, 40, &mut ranges);
        let pieces: Vec<&str> = ranges
            .iter()
            .map(|&(start, end)| &text[start..end])
            .collect();
        assert_eq!(pieces, vec!["une", "ligne"]);

        let text = "aaaaaaaaaa";
        wrap_text(text, 4, &mut ranges);
        let pieces: Vec<&str> = ranges
            .iter()
            .map(|&(start, end)| &text[start..end])
            .collect();
        assert_eq!(pieces, vec!["aaaa", "aaaa", "aa"]);

        wrap_text("", 10, &mut ranges);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn narrow_viewport_keeps_the_layout_sane() {
        let fixture = Fixture::new("etroit");
        let mut hud = Hud::new();
        hud.build(
            HudViewport {
                width: 12.0,
                height: 8.0,
            },
            &fixture.model(),
        );
        let editor = hud.editor_rect();
        assert!(editor.w >= 0.0);
        assert!(editor.h >= 0.0);
        for quad in hud.quads() {
            assert!(quad.rect.w > 0.0);
            assert!(quad.rect.h > 0.0);
        }
        assert_eq!(hud.hit(-1.0, -1.0), Target::None);
        assert_eq!(hud.hit(500.0, 500.0), Target::None);
    }

    #[test]
    fn scrollbars_answer_when_the_tree_overflows() {
        let mut fixture = Fixture::new("ascenseur");
        for index in 0..80 {
            fs::write(
                fixture.sandbox.root.join(format!("f{:03}.rs", index)),
                "fn f() {}\n",
            )
            .expect("fichier de test");
        }
        fixture.project = Project::open(&fixture.sandbox.root);
        let mut hud = Hud::new();
        fixture.build(&mut hud);
        assert_eq!(
            hud.hit(SIDEBAR_WIDTH - 0.001, TAB_BAR_HEIGHT + 1.0),
            Target::Scrollbar(Region::Tree)
        );
        assert_eq!(
            hud.hit(
                SIDEBAR_WIDTH - SCROLLBAR_WIDTH - 0.001,
                TAB_BAR_HEIGHT + 1.0
            ),
            Target::TreeRow(0)
        );
    }
}
