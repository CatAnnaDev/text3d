use std::path::PathBuf;

use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

use crate::App;
use crate::hud::{Rect, Surface};
use crate::overlay::Panel;
use crate::tasks::Task;

pub const HINT_LIFETIME: f32 = 3.0;
pub const HINT_THICKNESS: f32 = 0.9;
pub const HINT_LAYER: u8 = 6;
pub const EDGE_COUNT: usize = 4;

const HINT_HOLD: f32 = 2.0;
const HINT_PEAK: f32 = 240.0;
const HINT_STEP: u8 = 0xF0;
const SUCCESS_COLOR: [u8; 3] = [126, 210, 160];
const ERROR_COLOR: [u8; 3] = [232, 104, 104];
const INFORMATION_COLOR: [u8; 3] = [102, 199, 245];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn of(surface: Surface) -> Option<Edge> {
        match surface {
            Surface::Tree => Some(Edge::Left),
            Surface::Problems => Some(Edge::Right),
            Surface::Results => Some(Edge::Top),
            Surface::Output => Some(Edge::Bottom),
            Surface::Code | Surface::Tabs | Surface::Screen => None,
        }
    }

    pub fn slot(self) -> usize {
        match self {
            Edge::Left => 0,
            Edge::Right => 1,
            Edge::Top => 2,
            Edge::Bottom => 3,
        }
    }

    pub fn band(self, width: f32, height: f32) -> Rect {
        let thickness = HINT_THICKNESS.min(width * 0.5).min(height * 0.5).max(0.0);
        match self {
            Edge::Left => Rect::new(0.0, 0.0, thickness, height),
            Edge::Right => Rect::new(width - thickness, 0.0, thickness, height),
            Edge::Top => Rect::new(0.0, 0.0, width, thickness),
            Edge::Bottom => Rect::new(0.0, height - thickness, width, thickness),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Success,
    Error,
    Information,
}

impl Tone {
    pub fn color(self) -> [u8; 3] {
        match self {
            Tone::Success => SUCCESS_COLOR,
            Tone::Error => ERROR_COLOR,
            Tone::Information => INFORMATION_COLOR,
        }
    }
}

pub fn hint_alpha(age: f32) -> u8 {
    if !(age >= 0.0) || age >= HINT_LIFETIME {
        return 0;
    }
    let fade = if age <= HINT_HOLD {
        1.0
    } else {
        (HINT_LIFETIME - age) / (HINT_LIFETIME - HINT_HOLD)
    };
    ((fade * HINT_PEAK) as u8) & HINT_STEP
}

pub fn surface_of_panel(panel: Panel) -> Option<Surface> {
    match panel {
        Panel::Problems => Some(Surface::Problems),
        Panel::Search | Panel::References => Some(Surface::Results),
        Panel::None
        | Panel::QuickOpen
        | Panel::DocumentSymbols
        | Panel::WorkspaceSymbols
        | Panel::Rename
        | Panel::Commands => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    OpenProject,
    QuickOpen,
    DocumentSymbols,
    WorkspaceSymbols,
    SearchProject,
    ToggleSidebar,
    ToggleOutput,
    Problems,
    GotoDefinition,
    FindReferences,
    Rename,
    Format,
    Build,
    Rebuild,
    Run,
    Test,
    Clippy,
    StopTask,
    CloseTab,
    NextTab,
    PreviousTab,
    TabAt(usize),
    JumpBack,
    JumpForward,
    Save,
    SaveAll,
    ExportObj,
    ExportGlb,
    ExportPng,
    ToggleShadows,
    ToggleWave,
    ToggleGrid,
    ToggleBevel,
    ToggleIndentDepth,
    ToggleLineNumbers,
    NextFont,
    Recenter,
    FaceTree,
    FaceProblems,
    FaceOutput,
    FaceResults,
    FaceCode,
    Quit,
}

impl Command {
    pub fn task(self) -> Option<Task> {
        match self {
            Command::Build => Some(Task::Check),
            Command::Rebuild => Some(Task::Build),
            Command::Run => Some(Task::Run),
            Command::Test => Some(Task::Test),
            Command::Clippy => Some(Task::Clippy),
            _ => None,
        }
    }

    pub fn surface(self) -> Option<Surface> {
        match self {
            Command::FaceTree => Some(Surface::Tree),
            Command::FaceProblems => Some(Surface::Problems),
            Command::FaceOutput => Some(Surface::Output),
            Command::FaceResults => Some(Surface::Results),
            Command::FaceCode => Some(Surface::Code),
            _ => None,
        }
    }

    pub fn panel(self) -> Option<Panel> {
        match self {
            Command::QuickOpen => Some(Panel::QuickOpen),
            Command::DocumentSymbols => Some(Panel::DocumentSymbols),
            Command::WorkspaceSymbols => Some(Panel::WorkspaceSymbols),
            Command::SearchProject => Some(Panel::Search),
            Command::Problems => Some(Panel::Problems),
            Command::Rename => Some(Panel::Rename),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Chord {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

impl Chord {
    pub fn from_state(state: ModifiersState) -> Chord {
        Chord {
            command: state.super_key(),
            shift: state.shift_key(),
            alt: state.alt_key(),
            control: state.control_key(),
        }
    }

    pub fn bare(&self) -> bool {
        !self.command && !self.control && !self.alt
    }
}

static ALL: [Command; 42] = [
    Command::OpenProject,
    Command::QuickOpen,
    Command::DocumentSymbols,
    Command::WorkspaceSymbols,
    Command::SearchProject,
    Command::ToggleSidebar,
    Command::ToggleOutput,
    Command::Problems,
    Command::GotoDefinition,
    Command::FindReferences,
    Command::Rename,
    Command::Format,
    Command::Build,
    Command::Rebuild,
    Command::Run,
    Command::Test,
    Command::Clippy,
    Command::StopTask,
    Command::CloseTab,
    Command::NextTab,
    Command::PreviousTab,
    Command::JumpBack,
    Command::JumpForward,
    Command::Save,
    Command::SaveAll,
    Command::ExportObj,
    Command::ExportGlb,
    Command::ExportPng,
    Command::ToggleShadows,
    Command::ToggleWave,
    Command::ToggleGrid,
    Command::ToggleBevel,
    Command::ToggleIndentDepth,
    Command::ToggleLineNumbers,
    Command::NextFont,
    Command::Recenter,
    Command::FaceTree,
    Command::FaceProblems,
    Command::FaceOutput,
    Command::FaceResults,
    Command::FaceCode,
    Command::Quit,
];

pub fn all() -> &'static [Command] {
    &ALL
}

pub fn label(command: Command) -> &'static str {
    match command {
        Command::OpenProject => "ouvrir un projet",
        Command::QuickOpen => "ouverture rapide de fichier",
        Command::DocumentSymbols => "symboles du document",
        Command::WorkspaceSymbols => "symboles du projet",
        Command::SearchProject => "rechercher dans le projet",
        Command::ToggleSidebar => "arbre de fichiers",
        Command::ToggleOutput => "panneau de sortie",
        Command::Problems => "problemes",
        Command::GotoDefinition => "aller a la definition",
        Command::FindReferences => "references du symbole",
        Command::Rename => "renommer le symbole",
        Command::Format => "formater le document",
        Command::Build => "verifier",
        Command::Rebuild => "compiler",
        Command::Run => "lancer",
        Command::Test => "tester",
        Command::Clippy => "clippy",
        Command::StopTask => "arreter la tache",
        Command::CloseTab => "fermer l onglet",
        Command::NextTab => "onglet suivant",
        Command::PreviousTab => "onglet precedent",
        Command::TabAt(_) => "aller a l onglet",
        Command::JumpBack => "retour au saut precedent",
        Command::JumpForward => "avancer dans les sauts",
        Command::Save => "enregistrer",
        Command::SaveAll => "tout enregistrer",
        Command::ExportObj => "exporter le maillage obj",
        Command::ExportGlb => "exporter le maillage glb",
        Command::ExportPng => "capture png",
        Command::ToggleShadows => "ombres portees",
        Command::ToggleWave => "ondulation",
        Command::ToggleGrid => "grille",
        Command::ToggleBevel => "biseau",
        Command::ToggleIndentDepth => "relief par indentation",
        Command::ToggleLineNumbers => "numeros de ligne",
        Command::NextFont => "fonte suivante",
        Command::Recenter => "recadrer la vue",
        Command::FaceTree => "viser l arbre de fichiers",
        Command::FaceProblems => "viser les problemes",
        Command::FaceOutput => "viser le terminal",
        Command::FaceResults => "viser les resultats",
        Command::FaceCode => "revenir au code",
        Command::Quit => "quitter",
    }
}

pub fn shortcut(command: Command) -> &'static str {
    match command {
        Command::OpenProject => "cmd+o",
        Command::QuickOpen => "cmd+p",
        Command::DocumentSymbols => "cmd+shift+o",
        Command::WorkspaceSymbols => "cmd+t",
        Command::SearchProject => "cmd+shift+f",
        Command::ToggleSidebar => "cmd+shift+e",
        Command::ToggleOutput => "ctrl+'",
        Command::Problems => "cmd+shift+m",
        Command::GotoDefinition => "f12",
        Command::FindReferences => "shift+f12",
        Command::Rename => "f2",
        Command::Format => "shift+option+f",
        Command::Build => "cmd+b",
        Command::Rebuild => "cmd+shift+b",
        Command::Run => "cmd+r",
        Command::Test => "cmd+shift+t",
        Command::Clippy => "cmd+shift+k",
        Command::StopTask => "cmd+point",
        Command::CloseTab => "cmd+w",
        Command::NextTab => "ctrl+tab",
        Command::PreviousTab => "ctrl+shift+tab",
        Command::TabAt(_) => "cmd+1 a cmd+9",
        Command::JumpBack => "ctrl+option+gauche",
        Command::JumpForward => "ctrl+option+droite",
        Command::Save => "cmd+s",
        Command::SaveAll => "cmd+option+s",
        Command::ExportObj => "option+e",
        Command::ExportGlb => "option+shift+e",
        Command::ExportPng => "option+p",
        Command::ToggleShadows => "option+4",
        Command::ToggleWave => "option+2",
        Command::ToggleGrid => "option+3",
        Command::ToggleBevel => "option+6",
        Command::ToggleIndentDepth => "option+7",
        Command::ToggleLineNumbers => "option+8",
        Command::NextFont => "option+5",
        Command::Recenter => "option+1",
        Command::FaceTree => "option+gauche",
        Command::FaceProblems => "option+droite",
        Command::FaceOutput => "option+bas",
        Command::FaceResults => "option+haut",
        Command::FaceCode => "option+entree",
        Command::Quit => "cmd+q",
    }
}

pub fn resolve(key: &Key, physical: PhysicalKey, chord: Chord) -> Option<Command> {
    if chord.command && chord.control {
        return None;
    }
    if let Some(command) = function_key(key, chord) {
        return Some(command);
    }
    if chord.command {
        return command_chord(key, physical, chord);
    }
    if chord.control {
        return control_chord(key, physical, chord);
    }
    if chord.alt {
        return option_chord(key, physical, chord);
    }
    None
}

fn function_key(key: &Key, chord: Chord) -> Option<Command> {
    if !chord.bare() {
        return None;
    }
    match key.as_ref() {
        Key::Named(NamedKey::F12) if chord.shift => Some(Command::FindReferences),
        Key::Named(NamedKey::F12) => Some(Command::GotoDefinition),
        Key::Named(NamedKey::F2) => Some(Command::Rename),
        _ => None,
    }
}

fn command_chord(key: &Key, physical: PhysicalKey, chord: Chord) -> Option<Command> {
    if matches!(key.as_ref(), Key::Named(_)) {
        return None;
    }
    if chord.alt {
        return match letter_of(key).or_else(|| physical_letter(physical)) {
            Some('s') => Some(Command::SaveAll),
            _ => None,
        };
    }
    if let Some(digit) = digit_of(key, physical) {
        return Some(Command::TabAt(digit as usize - 1));
    }
    if is_period(key, physical) {
        return Some(Command::StopTask);
    }
    let letter = letter_of(key).or_else(|| physical_letter(physical))?;
    match (letter, chord.shift) {
        ('o', false) => Some(Command::OpenProject),
        ('o', true) => Some(Command::DocumentSymbols),
        ('p', false) => Some(Command::QuickOpen),
        ('t', false) => Some(Command::WorkspaceSymbols),
        ('t', true) => Some(Command::Test),
        ('f', true) => Some(Command::SearchProject),
        ('e', true) => Some(Command::ToggleSidebar),
        ('m', true) => Some(Command::Problems),
        ('b', false) => Some(Command::Build),
        ('b', true) => Some(Command::Rebuild),
        ('r', false) => Some(Command::Run),
        ('k', true) => Some(Command::Clippy),
        ('w', false) => Some(Command::CloseTab),
        ('s', false) => Some(Command::Save),
        ('q', false) => Some(Command::Quit),
        _ => None,
    }
}

fn control_chord(key: &Key, physical: PhysicalKey, chord: Chord) -> Option<Command> {
    match key.as_ref() {
        Key::Named(NamedKey::Tab) if !chord.alt && chord.shift => Some(Command::PreviousTab),
        Key::Named(NamedKey::Tab) if !chord.alt => Some(Command::NextTab),
        Key::Named(NamedKey::ArrowLeft) if chord.alt => Some(Command::JumpBack),
        Key::Named(NamedKey::ArrowRight) if chord.alt => Some(Command::JumpForward),
        Key::Named(_) => None,
        _ if chord.alt => None,
        Key::Character(written) if written == "'" => Some(Command::ToggleOutput),
        _ if matches!(physical, PhysicalKey::Code(KeyCode::Quote)) => Some(Command::ToggleOutput),
        _ => None,
    }
}

fn option_chord(key: &Key, physical: PhysicalKey, chord: Chord) -> Option<Command> {
    if let Key::Named(named) = key.as_ref() {
        if chord.shift {
            return None;
        }
        return match named {
            NamedKey::ArrowLeft => Some(Command::FaceTree),
            NamedKey::ArrowRight => Some(Command::FaceProblems),
            NamedKey::ArrowDown => Some(Command::FaceOutput),
            NamedKey::ArrowUp => Some(Command::FaceResults),
            NamedKey::Enter => Some(Command::FaceCode),
            _ => None,
        };
    }
    if let Some(digit) = digit_of(key, physical) {
        return match digit {
            1 => Some(Command::Recenter),
            2 => Some(Command::ToggleWave),
            3 => Some(Command::ToggleGrid),
            4 => Some(Command::ToggleShadows),
            5 => Some(Command::NextFont),
            6 => Some(Command::ToggleBevel),
            7 => Some(Command::ToggleIndentDepth),
            8 => Some(Command::ToggleLineNumbers),
            _ => None,
        };
    }
    let letter = physical_letter(physical).or_else(|| letter_of(key))?;
    match (letter, chord.shift) {
        ('e', false) => Some(Command::ExportObj),
        ('e', true) => Some(Command::ExportGlb),
        ('p', false) => Some(Command::ExportPng),
        ('f', true) => Some(Command::Format),
        _ => None,
    }
}

pub fn letter_of(key: &Key) -> Option<char> {
    let Key::Character(written) = key.as_ref() else {
        return None;
    };
    let mut chars = written.chars();
    let first = chars.next()?;
    if chars.next().is_some() || !first.is_ascii_alphabetic() {
        return None;
    }
    Some(first.to_ascii_lowercase())
}

fn digit_of(key: &Key, physical: PhysicalKey) -> Option<u32> {
    if let PhysicalKey::Code(code) = physical {
        let value = match code {
            KeyCode::Digit1 | KeyCode::Numpad1 => 1,
            KeyCode::Digit2 | KeyCode::Numpad2 => 2,
            KeyCode::Digit3 | KeyCode::Numpad3 => 3,
            KeyCode::Digit4 | KeyCode::Numpad4 => 4,
            KeyCode::Digit5 | KeyCode::Numpad5 => 5,
            KeyCode::Digit6 | KeyCode::Numpad6 => 6,
            KeyCode::Digit7 | KeyCode::Numpad7 => 7,
            KeyCode::Digit8 | KeyCode::Numpad8 => 8,
            KeyCode::Digit9 | KeyCode::Numpad9 => 9,
            _ => 0,
        };
        if value != 0 {
            return Some(value);
        }
    }
    let Key::Character(written) = key.as_ref() else {
        return None;
    };
    let mut chars = written.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match first {
        '1'..='9' => Some(first as u32 - '0' as u32),
        _ => None,
    }
}

fn is_period(key: &Key, physical: PhysicalKey) -> bool {
    if matches!(physical, PhysicalKey::Code(KeyCode::Period) | PhysicalKey::Code(KeyCode::NumpadDecimal)) {
        return true;
    }
    matches!(key.as_ref(), Key::Character(written) if written == ".")
}

fn physical_letter(physical: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    let letter = match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    };
    Some(letter)
}

pub fn completion_trigger(previous: Option<char>, typed: char) -> Option<char> {
    match typed {
        '.' => Some('.'),
        ':' => Some(':'),
        '>' if previous == Some('-') => Some('>'),
        _ => None,
    }
}

pub fn location_in_line(line: &str) -> Option<(PathBuf, usize, usize)> {
    if let Some(found) = bracket_location(line) {
        return Some(found);
    }
    colon_location(line)
}

fn bracket_location(line: &str) -> Option<(PathBuf, usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b')' {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) != Some(&b':') {
            index += 1;
            continue;
        }
        let Some(open) = line[..index].rfind('(') else {
            index += 1;
            continue;
        };
        let inside = &line[open + 1..index];
        let Some((left, right)) = inside.split_once(',') else {
            index += 1;
            continue;
        };
        let (Ok(row), Ok(column)) = (left.trim().parse::<usize>(), right.trim().parse::<usize>())
        else {
            index += 1;
            continue;
        };
        let path = line[..open].trim();
        if path.is_empty() {
            index += 1;
            continue;
        }
        return Some((PathBuf::from(path), row.max(1), column.max(1)));
    }
    None
}

fn colon_location(line: &str) -> Option<(PathBuf, usize, usize)> {
    for raw in line.split_whitespace() {
        let token = raw.trim_matches(|ch: char| {
            ch == ','
                || ch == ';'
                || ch == '\''
                || ch == '"'
                || ch == '('
                || ch == ')'
                || ch == '['
                || ch == ']'
                || ch == ':'
        });
        if token.len() < 3 {
            continue;
        }
        let mut pieces = token.rsplitn(3, ':');
        let last = pieces.next().unwrap_or("");
        let Some(middle) = pieces.next() else {
            continue;
        };
        match pieces.next() {
            Some(head) if is_number(last) && is_number(middle) && looks_like_path(head) => {
                let row = last_number(middle);
                let column = last_number(last);
                return Some((PathBuf::from(head), row, column));
            }
            _ => {
                if is_number(last) && looks_like_path(middle) {
                    return Some((PathBuf::from(middle), last_number(last), 1));
                }
            }
        }
    }
    None
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn last_number(text: &str) -> usize {
    text.parse::<usize>().unwrap_or(1).max(1)
}

fn looks_like_path(text: &str) -> bool {
    if text.is_empty() || is_number(text) {
        return false;
    }
    text.contains('.') || text.contains('/')
}

pub fn execute(app: &mut App, command: Command, event_loop: &ActiveEventLoop) {
    if let Some(panel) = command.panel()
        && app.showing(panel)
    {
        app.close_panel();
        return;
    }
    if let Some(surface) = command.surface() {
        app.aim_at(surface);
        return;
    }
    if let Some(task) = command.task() {
        app.run_task(task);
        return;
    }
    match command {
        Command::OpenProject => app.open_project_dialog(),
        Command::QuickOpen => app.open_quick_open(),
        Command::DocumentSymbols => app.open_document_symbols(),
        Command::WorkspaceSymbols => app.open_workspace_symbols(),
        Command::SearchProject => app.open_project_search(),
        Command::ToggleSidebar => app.toggle_sidebar(),
        Command::ToggleOutput => app.toggle_output(),
        Command::Problems => app.open_problems(),
        Command::GotoDefinition => app.request_definition(),
        Command::FindReferences => app.request_references(),
        Command::Rename => app.open_rename(),
        Command::Format => app.request_format(),
        Command::StopTask => app.stop_task(),
        Command::CloseTab => app.close_active_tab(),
        Command::NextTab => app.cycle_tab(true),
        Command::PreviousTab => app.cycle_tab(false),
        Command::TabAt(index) => app.activate_tab(index),
        Command::JumpBack => app.jump(false),
        Command::JumpForward => app.jump(true),
        Command::Save => app.save_active(),
        Command::SaveAll => app.save_all(),
        Command::ExportObj => app.export_mesh(false),
        Command::ExportGlb => app.export_mesh(true),
        Command::ExportPng => app.capture_png(),
        Command::ToggleShadows => app.toggle_shadows(),
        Command::ToggleWave => app.toggle_wave(),
        Command::ToggleGrid => app.toggle_grid(),
        Command::ToggleBevel => app.toggle_bevel(),
        Command::ToggleIndentDepth => app.toggle_indent_depth(),
        Command::ToggleLineNumbers => app.toggle_line_numbers(),
        Command::NextFont => app.next_font(),
        Command::Recenter => app.recenter(),
        Command::Quit => app.quit(event_loop),
        Command::Build | Command::Rebuild | Command::Run | Command::Test | Command::Clippy => {}
        Command::FaceTree
        | Command::FaceProblems
        | Command::FaceOutput
        | Command::FaceResults
        | Command::FaceCode => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn written(text: &str) -> Key {
        Key::Character(SmolStr::new(text))
    }

    fn at(code: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(code)
    }

    fn cmd() -> Chord {
        Chord { command: true, ..Chord::default() }
    }

    fn cmd_shift() -> Chord {
        Chord { command: true, shift: true, ..Chord::default() }
    }

    fn option() -> Chord {
        Chord { alt: true, ..Chord::default() }
    }

    fn control() -> Chord {
        Chord { control: true, ..Chord::default() }
    }

    #[test]
    fn raccourcis_de_projet() {
        assert_eq!(
            resolve(&written("o"), at(KeyCode::KeyO), cmd()),
            Some(Command::OpenProject)
        );
        assert_eq!(
            resolve(&written("p"), at(KeyCode::KeyP), cmd()),
            Some(Command::QuickOpen)
        );
        assert_eq!(
            resolve(&written("O"), at(KeyCode::KeyO), cmd_shift()),
            Some(Command::DocumentSymbols)
        );
        assert_eq!(
            resolve(&written("t"), at(KeyCode::KeyT), cmd()),
            Some(Command::WorkspaceSymbols)
        );
        assert_eq!(
            resolve(&written("F"), at(KeyCode::KeyF), cmd_shift()),
            Some(Command::SearchProject)
        );
        assert_eq!(
            resolve(&written("E"), at(KeyCode::KeyE), cmd_shift()),
            Some(Command::ToggleSidebar)
        );
        assert_eq!(
            resolve(&written("M"), at(KeyCode::KeyM), cmd_shift()),
            Some(Command::Problems)
        );
    }

    #[test]
    fn la_palette_de_commandes_reste_hors_table() {
        assert_eq!(resolve(&written("P"), at(KeyCode::KeyP), cmd_shift()), None);
    }

    #[test]
    fn raccourcis_de_langage() {
        let bare = Chord::default();
        let shift = Chord { shift: true, ..Chord::default() };
        assert_eq!(
            resolve(&Key::Named(NamedKey::F12), at(KeyCode::F12), bare),
            Some(Command::GotoDefinition)
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::F12), at(KeyCode::F12), shift),
            Some(Command::FindReferences)
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::F2), at(KeyCode::F2), bare),
            Some(Command::Rename)
        );
        let shift_option = Chord { shift: true, alt: true, ..Chord::default() };
        assert_eq!(
            resolve(&written("Ï"), at(KeyCode::KeyF), shift_option),
            Some(Command::Format)
        );
    }

    #[test]
    fn raccourcis_de_taches() {
        assert_eq!(resolve(&written("b"), at(KeyCode::KeyB), cmd()), Some(Command::Build));
        assert_eq!(
            resolve(&written("B"), at(KeyCode::KeyB), cmd_shift()),
            Some(Command::Rebuild)
        );
        assert_eq!(resolve(&written("r"), at(KeyCode::KeyR), cmd()), Some(Command::Run));
        assert_eq!(
            resolve(&written("T"), at(KeyCode::KeyT), cmd_shift()),
            Some(Command::Test)
        );
        assert_eq!(
            resolve(&written("K"), at(KeyCode::KeyK), cmd_shift()),
            Some(Command::Clippy)
        );
        assert_eq!(
            resolve(&written("."), at(KeyCode::Period), cmd()),
            Some(Command::StopTask)
        );
        assert_eq!(Command::Build.task(), Some(Task::Check));
        assert_eq!(Command::Rebuild.task(), Some(Task::Build));
    }

    #[test]
    fn raccourcis_d_onglets() {
        assert_eq!(resolve(&written("w"), at(KeyCode::KeyW), cmd()), Some(Command::CloseTab));
        assert_eq!(
            resolve(&Key::Named(NamedKey::Tab), at(KeyCode::Tab), control()),
            Some(Command::NextTab)
        );
        let control_shift = Chord { control: true, shift: true, ..Chord::default() };
        assert_eq!(
            resolve(&Key::Named(NamedKey::Tab), at(KeyCode::Tab), control_shift),
            Some(Command::PreviousTab)
        );
        assert_eq!(
            resolve(&written("1"), at(KeyCode::Digit1), cmd()),
            Some(Command::TabAt(0))
        );
        assert_eq!(
            resolve(&written("9"), at(KeyCode::Digit9), cmd()),
            Some(Command::TabAt(8))
        );
        let control_option = Chord { control: true, alt: true, ..Chord::default() };
        assert_eq!(
            resolve(&Key::Named(NamedKey::ArrowLeft), at(KeyCode::ArrowLeft), control_option),
            Some(Command::JumpBack)
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::ArrowRight), at(KeyCode::ArrowRight), control_option),
            Some(Command::JumpForward)
        );
    }

    #[test]
    fn le_panneau_de_sortie_est_sur_ctrl_apostrophe() {
        assert_eq!(
            resolve(&written("'"), at(KeyCode::Quote), control()),
            Some(Command::ToggleOutput)
        );
        assert_eq!(
            resolve(&written("@"), at(KeyCode::Quote), control()),
            Some(Command::ToggleOutput)
        );
    }

    #[test]
    fn les_bascules_de_vue_sont_sur_option_chiffre() {
        let expected = [
            (KeyCode::Digit1, Command::Recenter),
            (KeyCode::Digit2, Command::ToggleWave),
            (KeyCode::Digit3, Command::ToggleGrid),
            (KeyCode::Digit4, Command::ToggleShadows),
            (KeyCode::Digit5, Command::NextFont),
            (KeyCode::Digit6, Command::ToggleBevel),
            (KeyCode::Digit7, Command::ToggleIndentDepth),
            (KeyCode::Digit8, Command::ToggleLineNumbers),
        ];
        for (code, command) in expected {
            assert_eq!(resolve(&written("¡"), at(code), option()), Some(command));
        }
        assert_eq!(resolve(&written("¡"), at(KeyCode::Digit9), option()), None);
    }

    #[test]
    fn les_exports_sont_sur_option_lettre() {
        assert_eq!(
            resolve(&written("´"), at(KeyCode::KeyE), option()),
            Some(Command::ExportObj)
        );
        let option_shift = Chord { alt: true, shift: true, ..Chord::default() };
        assert_eq!(
            resolve(&written("´"), at(KeyCode::KeyE), option_shift),
            Some(Command::ExportGlb)
        );
        assert_eq!(
            resolve(&written("π"), at(KeyCode::KeyP), option()),
            Some(Command::ExportPng)
        );
    }

    #[test]
    fn enregistrer_et_quitter() {
        assert_eq!(resolve(&written("s"), at(KeyCode::KeyS), cmd()), Some(Command::Save));
        let cmd_option = Chord { command: true, alt: true, ..Chord::default() };
        assert_eq!(
            resolve(&written("ß"), at(KeyCode::KeyS), cmd_option),
            Some(Command::SaveAll)
        );
        assert_eq!(resolve(&written("q"), at(KeyCode::KeyQ), cmd()), Some(Command::Quit));
    }

    #[test]
    fn la_frappe_simple_ne_declenche_aucune_commande() {
        let bare = Chord::default();
        assert_eq!(resolve(&written("a"), at(KeyCode::KeyA), bare), None);
        assert_eq!(resolve(&written("1"), at(KeyCode::Digit1), bare), None);
        assert_eq!(
            resolve(&Key::Named(NamedKey::Enter), at(KeyCode::Enter), bare),
            None
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::Space), at(KeyCode::Space), control()),
            None
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::ArrowLeft), at(KeyCode::ArrowLeft), cmd()),
            None
        );
    }

    #[test]
    fn les_libelles_sont_en_minuscules_sans_accent() {
        for command in all() {
            let text = label(*command);
            assert!(!text.is_empty());
            assert!(text.is_ascii(), "libelle non ascii: {text}");
            assert_eq!(text, text.to_lowercase(), "libelle non minuscule: {text}");
            let keys = shortcut(*command);
            assert!(!keys.is_empty());
            assert!(keys.is_ascii(), "raccourci non ascii: {keys}");
        }
    }

    #[test]
    fn la_table_est_complete_et_sans_doublon() {
        assert_eq!(all().len(), 42);
        for (index, command) in all().iter().enumerate() {
            for other in all().iter().skip(index + 1) {
                assert_ne!(command, other);
                assert_ne!(label(*command), label(*other));
            }
        }
    }

    #[test]
    fn les_panneaux_sont_relies() {
        assert!(Command::QuickOpen.panel() == Some(Panel::QuickOpen));
        assert!(Command::Problems.panel() == Some(Panel::Problems));
        assert!(Command::Rename.panel() == Some(Panel::Rename));
        assert!(Command::Save.panel().is_none());
    }

    #[test]
    fn la_visee_des_surfaces_est_sur_option_fleche() {
        let expected = [
            (NamedKey::ArrowLeft, KeyCode::ArrowLeft, Command::FaceTree),
            (NamedKey::ArrowRight, KeyCode::ArrowRight, Command::FaceProblems),
            (NamedKey::ArrowDown, KeyCode::ArrowDown, Command::FaceOutput),
            (NamedKey::ArrowUp, KeyCode::ArrowUp, Command::FaceResults),
            (NamedKey::Enter, KeyCode::Enter, Command::FaceCode),
        ];
        for (named, code, command) in expected {
            assert_eq!(resolve(&Key::Named(named), at(code), option()), Some(command));
        }
    }

    #[test]
    fn option_shift_fleche_reste_a_la_selection_par_mot() {
        let option_shift = Chord { alt: true, shift: true, ..Chord::default() };
        assert_eq!(
            resolve(&Key::Named(NamedKey::ArrowLeft), at(KeyCode::ArrowLeft), option_shift),
            None
        );
        let control_option = Chord { control: true, alt: true, ..Chord::default() };
        assert_eq!(
            resolve(&Key::Named(NamedKey::ArrowLeft), at(KeyCode::ArrowLeft), control_option),
            Some(Command::JumpBack)
        );
    }

    #[test]
    fn chaque_visee_porte_sa_surface() {
        assert_eq!(Command::FaceTree.surface(), Some(Surface::Tree));
        assert_eq!(Command::FaceProblems.surface(), Some(Surface::Problems));
        assert_eq!(Command::FaceOutput.surface(), Some(Surface::Output));
        assert_eq!(Command::FaceResults.surface(), Some(Surface::Results));
        assert_eq!(Command::FaceCode.surface(), Some(Surface::Code));
        assert_eq!(Command::Recenter.surface(), None);
        assert_eq!(Command::Build.surface(), None);
    }

    #[test]
    fn les_panneaux_meubles_ont_une_surface() {
        assert_eq!(surface_of_panel(Panel::Problems), Some(Surface::Problems));
        assert_eq!(surface_of_panel(Panel::Search), Some(Surface::Results));
        assert_eq!(surface_of_panel(Panel::References), Some(Surface::Results));
        assert_eq!(surface_of_panel(Panel::QuickOpen), None);
        assert_eq!(surface_of_panel(Panel::Commands), None);
        assert_eq!(surface_of_panel(Panel::Rename), None);
        assert_eq!(surface_of_panel(Panel::None), None);
    }

    #[test]
    fn chaque_mur_a_son_bord_d_ecran() {
        assert_eq!(Edge::of(Surface::Tree), Some(Edge::Left));
        assert_eq!(Edge::of(Surface::Problems), Some(Edge::Right));
        assert_eq!(Edge::of(Surface::Results), Some(Edge::Top));
        assert_eq!(Edge::of(Surface::Output), Some(Edge::Bottom));
        assert_eq!(Edge::of(Surface::Code), None);
        assert_eq!(Edge::of(Surface::Tabs), None);
        assert_eq!(Edge::of(Surface::Screen), None);
        let mut seen = [false; EDGE_COUNT];
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let slot = edge.slot();
            assert!(slot < EDGE_COUNT);
            assert!(!seen[slot]);
            seen[slot] = true;
        }
    }

    #[test]
    fn les_bandes_collent_au_bon_bord() {
        let width = 100.0;
        let height = 60.0;
        let left = Edge::Left.band(width, height);
        assert_eq!(left.x, 0.0);
        assert_eq!(left.h, height);
        assert!(left.w > 0.0 && left.w < width * 0.5);
        let right = Edge::Right.band(width, height);
        assert!((right.right() - width).abs() < 1.0e-4);
        assert_eq!(right.h, height);
        let top = Edge::Top.band(width, height);
        assert_eq!(top.y, 0.0);
        assert_eq!(top.w, width);
        let bottom = Edge::Bottom.band(width, height);
        assert!((bottom.bottom() - height).abs() < 1.0e-4);
        assert_eq!(bottom.w, width);
        let tiny = Edge::Left.band(0.2, 0.2);
        assert!(tiny.w <= 0.1 + 1.0e-4);
    }

    #[test]
    fn l_indice_peripherique_dure_trois_secondes() {
        assert!(hint_alpha(0.0) > 200);
        assert_eq!(hint_alpha(0.0), hint_alpha(1.5));
        assert_eq!(hint_alpha(HINT_LIFETIME), 0);
        assert_eq!(hint_alpha(HINT_LIFETIME + 1.0), 0);
        assert_eq!(hint_alpha(-1.0), 0);
        assert_eq!(hint_alpha(f32::NAN), 0);
        let mut previous = hint_alpha(2.0);
        let mut step = 1;
        while step <= 10 {
            let age = 2.0 + step as f32 * 0.1;
            let current = hint_alpha(age);
            assert!(current <= previous, "alpha remonte a {age}");
            previous = current;
            step += 1;
        }
        assert_eq!(previous, 0);
    }

    #[test]
    fn les_couleurs_d_indice_sont_distinctes() {
        assert_ne!(Tone::Success.color(), Tone::Error.color());
        assert_ne!(Tone::Success.color(), Tone::Information.color());
        assert_ne!(Tone::Error.color(), Tone::Information.color());
        assert!(Tone::Success.color()[1] > Tone::Success.color()[0]);
        assert!(Tone::Error.color()[0] > Tone::Error.color()[1]);
        assert!(Tone::Information.color()[2] > Tone::Information.color()[0]);
    }

    #[test]
    fn declencheurs_de_completion() {
        assert_eq!(completion_trigger(None, '.'), Some('.'));
        assert_eq!(completion_trigger(Some('a'), ':'), Some(':'));
        assert_eq!(completion_trigger(Some('-'), '>'), Some('>'));
        assert_eq!(completion_trigger(Some('a'), '>'), None);
        assert_eq!(completion_trigger(Some('a'), 'b'), None);
    }

    #[test]
    fn emplacement_dans_une_ligne_de_cargo() {
        let found = location_in_line("  --> src/main.rs:412:9");
        assert_eq!(found, Some((PathBuf::from("src/main.rs"), 412, 9)));
        let sans_colonne = location_in_line("erreur dans src/text.rs:12");
        assert_eq!(sans_colonne, Some((PathBuf::from("src/text.rs"), 12, 1)));
    }

    #[test]
    fn emplacement_dans_une_ligne_de_dotnet() {
        let found = location_in_line(
            "/Users/anna/mon projet/Deep Profiler.cs(12,34): error CS1002: attendu [/x/P.csproj]",
        );
        assert_eq!(
            found,
            Some((PathBuf::from("/Users/anna/mon projet/Deep Profiler.cs"), 12, 34))
        );
    }

    #[test]
    fn une_ligne_sans_emplacement_ne_rend_rien() {
        assert_eq!(location_in_line("compilation terminee"), None);
        assert_eq!(location_in_line("12:30:45 demarrage"), None);
        assert_eq!(location_in_line(""), None);
    }
}
