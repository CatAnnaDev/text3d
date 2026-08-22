use std::fmt::Write as FmtWrite;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::lsp::position::{char_to_utf16, utf16_to_char};
use crate::lsp::protocol::{
    CompletionItem, Diagnostic, Location, Position, Range, Severity, SignatureInfo, Symbol, TextEdit,
};
use crate::lsp::session::{Event, Server, ServerKind};
use crate::project::Project;
use crate::search::{FileFinder, ProjectSearch};
use crate::tasks::{Task, TaskRunner};
use crate::text::{Cursor, TextBuffer};
use crate::workspace::Workspace;

const MAX_DISK_BYTES: u64 = 16 * 1024 * 1024;
const FINDER_BATCH: usize = 8192;
const MAX_NOTICES: usize = 64;
const PREVIEW_CHARS: usize = 120;
const NO_SERVER: &str = "sans serveur de langage";
const NOT_READY: &str = "serveur de langage en cours de demarrage";

#[derive(Clone)]
pub enum Notice {
    Status(String),
    Diagnostics(PathBuf),
    Completion { items: Vec<CompletionItem>, incomplete: bool },
    Hover(String),
    Signature(Option<SignatureInfo>),
    Definition(Vec<Location>),
    References(Vec<Symbol>),
    DocumentSymbols(Vec<Symbol>),
    WorkspaceSymbols(Vec<Symbol>),
    Renamed { files: usize, edits: usize },
    Formatted(usize),
    TaskFinished { label: String, code: Option<i32> },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Intent {
    Completion,
    Hover,
    Signature,
    Definition,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    Rename,
    Formatting,
}

impl Intent {
    fn reports(self) -> bool {
        !matches!(self, Intent::Completion | Intent::Hover | Intent::Signature)
    }

    fn versioned(self) -> bool {
        self != Intent::WorkspaceSymbols
    }
}

struct Request {
    slot: usize,
    id: u64,
    intent: Intent,
    path: PathBuf,
    version: i32,
}

struct Session {
    kind: ServerKind,
    root: PathBuf,
    server: Option<Server>,
    status: String,
    docs: Vec<PathBuf>,
}

struct DiagFile {
    path: PathBuf,
    lsp: Vec<Diagnostic>,
    task: Vec<Diagnostic>,
    merged: Vec<Diagnostic>,
    stale: bool,
    notify: bool,
}

struct DiskLines {
    path: PathBuf,
    text: String,
    starts: Vec<u32>,
    stamp: Option<SystemTime>,
    ascii: bool,
    valid: bool,
}

impl DiskLines {
    fn new() -> DiskLines {
        DiskLines {
            path: PathBuf::new(),
            text: String::new(),
            starts: Vec::new(),
            stamp: None,
            ascii: true,
            valid: false,
        }
    }

    fn load(&mut self, path: &Path) -> &DiskLines {
        let stamp = match std::fs::metadata(path) {
            Ok(data) if data.is_file() && data.len() <= MAX_DISK_BYTES => data.modified().ok(),
            _ => None,
        };
        if self.valid && stamp.is_some() && self.stamp == stamp && self.path == path {
            return self;
        }
        self.path.clear();
        self.path.push(path);
        self.text.clear();
        self.starts.clear();
        self.stamp = stamp;
        self.valid = false;
        self.ascii = true;
        if stamp.is_none() {
            return self;
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            return self;
        };
        if file.read_to_string(&mut self.text).is_err() {
            self.text.clear();
            return self;
        }
        self.valid = true;
        self.ascii = self.text.is_ascii();
        self.starts.reserve(self.text.len() / 24 + 1);
        self.starts.push(0);
        for (index, byte) in self.text.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                self.starts.push(index as u32 + 1);
            }
        }
        self
    }

    fn line(&self, index: usize) -> Option<&str> {
        if !self.valid {
            return None;
        }
        let start = *self.starts.get(index)? as usize;
        let end = match self.starts.get(index + 1) {
            Some(next) => (*next as usize).saturating_sub(1),
            None => self.text.len(),
        };
        let mut slice = self.text.get(start..end)?;
        if slice.ends_with('\r') {
            slice = &slice[..slice.len() - 1];
        }
        Some(slice)
    }
}

pub struct Ide {
    project: Project,
    workspace: Workspace,
    tasks: TaskRunner,
    search: ProjectSearch,
    finder: FileFinder,
    sessions: Vec<Session>,
    requests: Vec<Request>,
    diagnostics: Vec<DiagFile>,
    flat: Vec<(PathBuf, Diagnostic)>,
    errors: usize,
    warnings: usize,
    dirty: bool,
    status: String,
    scratch: String,
    text: String,
    events: Vec<Event>,
    pending: Vec<Notice>,
    saved: Vec<bool>,
    disk: DiskLines,
    task_diagnostics: usize,
    task_running: bool,
    indexed: usize,
    scanning: bool,
    searching: bool,
}

impl Ide {
    pub fn new(project: Project, workspace: Workspace) -> Ide {
        let mut ide = Ide {
            project,
            workspace,
            tasks: TaskRunner::new(),
            search: ProjectSearch::new(),
            finder: FileFinder::new(),
            sessions: Vec::new(),
            requests: Vec::new(),
            diagnostics: Vec::new(),
            flat: Vec::new(),
            errors: 0,
            warnings: 0,
            dirty: false,
            status: String::new(),
            scratch: String::new(),
            text: String::new(),
            events: Vec::new(),
            pending: Vec::new(),
            saved: Vec::new(),
            disk: DiskLines::new(),
            task_diagnostics: 0,
            task_running: false,
            indexed: 0,
            scanning: true,
            searching: false,
        };
        ide.finder.set_files(ide.project.files(), ide.project.root());
        ide.indexed = ide.project.files().len();
        for index in 0..ide.workspace.len() {
            ide.attach(index);
        }
        ide.mark();
        ide.announce_status();
        ide
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn project_mut(&mut self) -> &mut Project {
        &mut self.project
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    pub fn tasks(&self) -> &TaskRunner {
        &self.tasks
    }

    pub fn search(&self) -> &ProjectSearch {
        &self.search
    }

    pub fn finder_mut(&mut self) -> &mut FileFinder {
        &mut self.finder
    }

    pub fn poll(&mut self, out: &mut Vec<Notice>) {
        if !self.pending.is_empty() {
            out.append(&mut self.pending);
        }
        let mut events = std::mem::take(&mut self.events);
        for slot in 0..self.sessions.len() {
            events.clear();
            if let Some(server) = self.sessions[slot].server.as_mut() {
                server.poll(&mut events);
            }
            for event in events.drain(..) {
                self.absorb(slot, event, out);
            }
        }
        self.events = events;

        let scanning = self.project.poll();
        let count = self.project.files().len();
        if count != self.indexed && (!scanning || count >= self.indexed + FINDER_BATCH) {
            self.indexed = count;
            self.finder.set_files(self.project.files(), self.project.root());
        }
        if self.scanning && !scanning {
            self.scanning = false;
            let (files, _) = self.project.indexed();
            let mut line = String::new();
            let _ = write!(line, "{files} fichiers indexes");
            if self.project.truncated() {
                line.push_str(" (liste tronquee)");
            }
            out.push(Notice::Status(line));
        }
        if scanning {
            self.scanning = true;
        }

        let running = self.tasks.poll();
        self.absorb_tasks();
        if self.task_running && !running {
            self.task_running = false;
            let label = self.tasks.label().to_string();
            out.push(Notice::TaskFinished { label, code: self.tasks.exit_code() });
        }

        self.search.poll();
        if self.searching && !self.search.running() {
            self.searching = false;
            let mut line = String::new();
            let _ = write!(line, "recherche terminee: {} resultats", self.search.hits().len());
            if self.search.capped() {
                line.push_str(" (limite atteinte)");
            }
            out.push(Notice::Status(line));
        }

        self.settle(out);
        if self.refresh_status() {
            let text = self.status.clone();
            out.push(Notice::Status(text));
        }
    }

    pub fn open_project(&mut self, root: &Path) -> Result<(), String> {
        let full = std::fs::canonicalize(root)
            .map_err(|error| format!("dossier de projet illisible: {error}"))?;
        if !full.is_dir() {
            return Err("le chemin choisi n est pas un dossier".to_string());
        }
        self.tasks.kill();
        self.tasks.clear();
        self.task_diagnostics = 0;
        self.task_running = false;
        self.search.cancel();
        self.searching = false;
        self.requests.clear();
        for session in self.sessions.iter_mut() {
            if let Some(server) = session.server.as_mut() {
                server.stop();
            }
            session.docs.clear();
        }
        self.sessions.clear();
        self.diagnostics.clear();
        self.flat.clear();
        self.errors = 0;
        self.warnings = 0;
        self.dirty = false;
        self.project = Project::open(&full);
        self.finder.set_files(self.project.files(), self.project.root());
        self.indexed = self.project.files().len();
        self.scanning = true;
        for index in 0..self.workspace.len() {
            self.attach(index);
        }
        self.mark();
        self.announce_status();
        Ok(())
    }

    pub fn open_file(&mut self, path: &Path, cursor: Option<Cursor>) -> Result<(), String> {
        let index = self.workspace.open(path)?;
        if let Some(cursor) = cursor {
            self.workspace.buffer_mut().set_cursor(cursor, false);
        }
        self.project.expand_to(path);
        self.attach(index);
        self.mark();
        self.announce_status();
        Ok(())
    }

    pub fn close_tab(&mut self, index: usize, force: bool) -> Result<(), String> {
        if index >= self.workspace.len() {
            return Err("cet onglet n existe pas".to_string());
        }
        let modified = self.workspace.buffer_at(index).modified;
        if modified && !force {
            return Err("modifications non enregistrees dans cet onglet".to_string());
        }
        if modified {
            self.workspace.activate(index);
            self.workspace.buffer_mut().modified = false;
        }
        let path = self.workspace.tabs().get(index).and_then(|tab| tab.path.clone());
        if !self.workspace.close(index) {
            return Err("fermeture de l onglet impossible".to_string());
        }
        if let Some(path) = path {
            self.detach(&path);
        }
        self.mark();
        self.announce_status();
        Ok(())
    }

    pub fn notify_edit(&mut self) {
        let index = self.workspace.active();
        self.workspace.bump_version(index);
        self.mark();
        let Some(path) = self.workspace.tabs().get(index).and_then(|tab| tab.path.as_deref())
        else {
            return;
        };
        let Some(slot) = self.slot_for_path(path) else {
            return;
        };
        let mut text = std::mem::take(&mut self.text);
        document_text(self.workspace.buffer_at(index), &mut text);
        if let Some(server) = self.sessions[slot].server.as_mut() {
            server.change(path, &text);
        }
        self.text = text;
    }

    pub fn notify_saved(&mut self) {
        let count = self.workspace.len();
        if self.saved.len() < count {
            self.saved.resize(count, false);
        }
        let mut text = std::mem::take(&mut self.text);
        for index in 0..count {
            if self.workspace.buffer_at(index).modified || !self.saved[index] {
                continue;
            }
            let Some(path) = self.workspace.tabs().get(index).and_then(|tab| tab.path.as_deref())
            else {
                continue;
            };
            let Some(slot) = self.slot_for_path(path) else {
                continue;
            };
            document_text(self.workspace.buffer_at(index), &mut text);
            if let Some(server) = self.sessions[slot].server.as_mut() {
                server.save(path, &text);
            }
        }
        self.text = text;
        self.mark();
    }

    pub fn notify_activated(&mut self) {
        self.mark();
        self.announce_status();
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[Diagnostic] {
        match self.diagnostics.binary_search_by(|entry| entry.path.as_path().cmp(path)) {
            Ok(index) => &self.diagnostics[index].merged,
            Err(_) => &[],
        }
    }

    pub fn diagnostics_on_line(&self, path: &Path, line: usize) -> &[Diagnostic] {
        let items = self.diagnostics_for(path);
        if items.is_empty() || line > u32::MAX as usize {
            return &items[..0];
        }
        let line = line as u32;
        let start = items.partition_point(|item| item.range.start.line < line);
        let end = start + items[start..].partition_point(|item| item.range.start.line <= line);
        &items[start..end]
    }

    pub fn diagnostic_counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }

    pub fn all_diagnostics(&self) -> &[(PathBuf, Diagnostic)] {
        &self.flat
    }

    pub fn server_status(&self) -> &str {
        &self.status
    }

    pub fn server_ready(&self) -> bool {
        let Some(path) = self.workspace.buffer().path.as_deref() else {
            return false;
        };
        let Some(slot) = self.slot_for_path(path) else {
            return false;
        };
        self.sessions[slot].server.as_ref().is_some_and(Server::ready)
    }

    pub fn ask_completion(&mut self, trigger: Option<char>) {
        self.send(Intent::Completion, trigger, "");
    }

    pub fn ask_hover(&mut self) {
        self.send(Intent::Hover, None, "");
    }

    pub fn ask_signature(&mut self) {
        self.send(Intent::Signature, None, "");
    }

    pub fn ask_definition(&mut self) {
        self.send(Intent::Definition, None, "");
    }

    pub fn ask_references(&mut self) {
        self.send(Intent::References, None, "");
    }

    pub fn ask_document_symbols(&mut self) {
        self.send(Intent::DocumentSymbols, None, "");
    }

    pub fn ask_workspace_symbols(&mut self, query: &str) {
        let active = self.workspace.buffer().path.as_deref().and_then(|path| self.slot_for_path(path));
        let slot = match active {
            Some(slot) if self.sessions[slot].server.as_ref().is_some_and(Server::ready) => slot,
            _ => {
                let ready = self
                    .sessions
                    .iter()
                    .position(|session| session.server.as_ref().is_some_and(Server::ready));
                match ready {
                    Some(slot) => slot,
                    None => {
                        self.report_missing(active);
                        return;
                    }
                }
            }
        };
        self.cancel_intent(slot, Intent::WorkspaceSymbols);
        let Some(server) = self.sessions[slot].server.as_mut() else {
            return;
        };
        let id = server.workspace_symbols(query);
        self.requests.push(Request {
            slot,
            id,
            intent: Intent::WorkspaceSymbols,
            path: PathBuf::new(),
            version: 0,
        });
    }

    pub fn ask_rename(&mut self, new_name: &str) {
        self.send(Intent::Rename, None, new_name);
    }

    pub fn ask_format(&mut self) {
        self.send(Intent::Formatting, None, "");
    }

    pub fn apply_edits(
        &mut self,
        edits: &[(PathBuf, Vec<TextEdit>)],
    ) -> Result<(usize, usize), String> {
        if edits.iter().all(|(_, list)| list.is_empty()) {
            return Ok((0, 0));
        }
        let mut targets: Vec<usize> = Vec::with_capacity(edits.len());
        for (path, list) in edits {
            if list.is_empty() {
                targets.push(usize::MAX);
                continue;
            }
            match self.workspace.open(path) {
                Ok(index) => targets.push(index),
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        let origin = self.workspace.active();
        let mut order: Vec<(u32, u32)> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        let mut files = 0usize;
        let mut applied = 0usize;
        for group in 0..edits.len() {
            let index = targets[group];
            if index == usize::MAX || seen.contains(&index) {
                continue;
            }
            seen.push(index);
            order.clear();
            for (other, target) in targets.iter().enumerate() {
                if *target != index {
                    continue;
                }
                for slot in 0..edits[other].1.len() {
                    order.push((other as u32, slot as u32));
                }
            }
            order.sort_unstable_by(|left, right| {
                let a = edits[left.0 as usize].1[left.1 as usize].range.start;
                let b = edits[right.0 as usize].1[right.1 as usize].range.start;
                b.line.cmp(&a.line).then(b.character.cmp(&a.character))
            });
            self.workspace.activate(index);
            self.attach(index);
            let buffer = self.workspace.buffer_mut();
            let mut count = 0usize;
            for (owner, slot) in order.iter() {
                let edit = &edits[*owner as usize].1[*slot as usize];
                if edit.text.is_empty() && edit.range.start == edit.range.end {
                    continue;
                }
                let start = cursor_at(buffer, edit.range.start);
                let end = cursor_at(buffer, edit.range.end);
                buffer.set_cursor(start, false);
                buffer.set_cursor(end, true);
                buffer.insert_text(&edit.text);
                count += 1;
            }
            if count > 0 {
                files += 1;
                applied += count;
                self.notify_edit();
            }
        }
        self.workspace.activate(origin);
        self.workspace.refresh();
        self.mark();
        Ok((files, applied))
    }

    pub fn run_task(&mut self, task: Task) -> Result<(), String> {
        if self.tasks.running() {
            return Err("une tache est deja en cours".to_string());
        }
        self.tasks.clear();
        self.task_diagnostics = 0;
        self.clear_task_diagnostics();
        self.tasks.start(task, &self.project)?;
        self.task_running = true;
        Ok(())
    }

    pub fn stop_task(&mut self) {
        if !self.tasks.running() {
            return;
        }
        self.tasks.kill();
    }

    pub fn start_search(&mut self, needle: &str, case_sensitive: bool, whole_word: bool) {
        self.search.start(self.project.files(), needle, case_sensitive, whole_word);
        self.searching = self.search.running();
    }

    pub fn shutdown(&mut self) {
        self.requests.clear();
        for session in self.sessions.iter_mut() {
            if let Some(server) = session.server.as_mut() {
                server.stop();
            }
            session.docs.clear();
        }
        self.tasks.kill();
        self.search.cancel();
    }

    fn absorb(&mut self, slot: usize, event: Event, out: &mut Vec<Notice>) {
        match event {
            Event::Ready | Event::Status(_) => {}
            Event::Diagnostics { path, items } => self.store_lsp(path, items),
            Event::Completion { id, items, incomplete } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                let mut items = items;
                self.convert_completion(&request.path, &mut items);
                out.push(Notice::Completion { items, incomplete });
            }
            Event::Hover { id, text } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) || text.is_empty() {
                    return;
                }
                out.push(Notice::Hover(text));
            }
            Event::Signature { id, info } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                out.push(Notice::Signature(info));
            }
            Event::Definition { id, locations } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                let mut locations = locations;
                self.convert_locations(&mut locations);
                out.push(Notice::Definition(locations));
            }
            Event::References { id, locations } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                let symbols = self.locations_to_symbols(locations);
                out.push(Notice::References(symbols));
            }
            Event::DocumentSymbols { id, items } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                let mut items = items;
                self.convert_symbols(&mut items);
                out.push(Notice::DocumentSymbols(items));
            }
            Event::WorkspaceSymbols { id, items } => {
                if self.take(slot, id).is_none() {
                    return;
                }
                let mut items = items;
                self.convert_symbols(&mut items);
                out.push(Notice::WorkspaceSymbols(items));
            }
            Event::Rename { id, edits } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    out.push(Notice::Failed(
                        "le document a change pendant le renommage".to_string(),
                    ));
                    return;
                }
                if edits.is_empty() {
                    out.push(Notice::Failed("aucun renommage possible ici".to_string()));
                    return;
                }
                match self.apply_edits(&edits) {
                    Ok((files, count)) => out.push(Notice::Renamed { files, edits: count }),
                    Err(message) => out.push(Notice::Failed(message)),
                }
            }
            Event::Formatting { id, edits } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if self.stale(&request) {
                    return;
                }
                if edits.is_empty() {
                    out.push(Notice::Formatted(0));
                    return;
                }
                let single = [(request.path.clone(), edits)];
                match self.apply_edits(&single) {
                    Ok((_, count)) => out.push(Notice::Formatted(count)),
                    Err(message) => out.push(Notice::Failed(message)),
                }
            }
            Event::Failed { id, message } => {
                let Some(request) = self.take(slot, id) else {
                    return;
                };
                if request.intent.reports() {
                    out.push(Notice::Failed(message));
                }
            }
            Event::Exited { message } => {
                let session = &mut self.sessions[slot];
                session.server = None;
                session.docs.clear();
                session.status.clear();
                let first = match message.lines().next() {
                    Some(line) if !line.is_empty() => line,
                    _ => "le serveur de langage s est arrete",
                };
                session.status.push_str(first);
                session.status.push_str(", sans intelligence de langage");
                self.requests.retain(|request| request.slot != slot);
                out.push(Notice::Failed(message));
            }
        }
    }

    fn settle(&mut self, out: &mut Vec<Notice>) {
        let mut emitted = 0usize;
        for index in 0..self.diagnostics.len() {
            if self.diagnostics[index].stale {
                self.diagnostics[index].stale = false;
                merge_entry(&mut self.diagnostics, index);
                self.dirty = true;
            }
            if self.diagnostics[index].notify {
                self.diagnostics[index].notify = false;
                if emitted < MAX_NOTICES {
                    emitted += 1;
                    let path = self.diagnostics[index].path.clone();
                    out.push(Notice::Diagnostics(path));
                }
            }
        }
        if self.dirty {
            self.dirty = false;
            self.rebuild_flat();
        }
    }

    fn store_lsp(&mut self, path: PathBuf, items: Vec<Diagnostic>) {
        let path = canonical(&path);
        let mut items = items;
        {
            let workspace = &self.workspace;
            let disk = &mut self.disk;
            for item in items.iter_mut() {
                convert_range(&mut item.range, &path, workspace, disk, true);
            }
        }
        let index = entry_of(&mut self.diagnostics, &path);
        self.diagnostics[index].lsp = items;
        self.diagnostics[index].stale = true;
        self.diagnostics[index].notify = true;
    }

    fn absorb_tasks(&mut self) {
        let produced = self.tasks.diagnostics().len();
        if produced < self.task_diagnostics {
            self.clear_task_diagnostics();
        }
        if produced == self.task_diagnostics {
            return;
        }
        let start = self.task_diagnostics;
        self.task_diagnostics = produced;
        let tasks = &self.tasks;
        let files = &mut self.diagnostics;
        let workspace = &self.workspace;
        let disk = &mut self.disk;
        for (path, item) in &tasks.diagnostics()[start..produced] {
            let index = entry_of(files, path);
            files[index].task.push(item.clone());
            let slot = files[index].task.len() - 1;
            let range = &mut files[index].task[slot].range;
            convert_range(range, path, workspace, disk, false);
            files[index].stale = true;
            files[index].notify = true;
        }
    }

    fn clear_task_diagnostics(&mut self) {
        for entry in self.diagnostics.iter_mut() {
            if entry.task.is_empty() {
                continue;
            }
            entry.task.clear();
            entry.stale = true;
            entry.notify = true;
        }
        self.task_diagnostics = 0;
    }

    fn rebuild_flat(&mut self) {
        let flat = &mut self.flat;
        let mut at = 0usize;
        let mut errors = 0usize;
        let mut warnings = 0usize;
        for entry in self.diagnostics.iter() {
            for item in entry.merged.iter() {
                match item.severity {
                    Severity::Error => errors += 1,
                    Severity::Warning => warnings += 1,
                    _ => {}
                }
                if at < flat.len() {
                    let slot = &mut flat[at];
                    if slot.0 != entry.path {
                        slot.0.clear();
                        slot.0.push(&entry.path);
                    }
                    assign_one(&mut slot.1, item);
                } else {
                    flat.push((entry.path.clone(), item.clone()));
                }
                at += 1;
            }
        }
        flat.truncate(at);
        self.errors = errors;
        self.warnings = warnings;
    }

    fn send(&mut self, intent: Intent, trigger: Option<char>, name: &str) {
        let Some((slot, position, version)) = self.begin(intent) else {
            return;
        };
        let index = self.workspace.active();
        let Some(path) = self.workspace.tabs().get(index).and_then(|tab| tab.path.as_deref())
        else {
            return;
        };
        let Some(server) = self.sessions[slot].server.as_mut() else {
            return;
        };
        let id = match intent {
            Intent::Completion => server.completion(path, position, trigger),
            Intent::Hover => server.hover(path, position),
            Intent::Signature => server.signature(path, position),
            Intent::Definition => server.definition(path, position),
            Intent::References => server.references(path, position),
            Intent::DocumentSymbols => server.document_symbols(path),
            Intent::Formatting => server.formatting(path),
            Intent::Rename => server.rename(path, position, name),
            Intent::WorkspaceSymbols => server.workspace_symbols(name),
        };
        let owned = path.to_path_buf();
        self.requests.push(Request { slot, id, intent, path: owned, version });
    }

    fn begin(&mut self, intent: Intent) -> Option<(usize, Position, i32)> {
        let index = self.workspace.active();
        let position;
        let found;
        {
            let buffer = self.workspace.buffer();
            let path = buffer.path.as_deref()?;
            let cursor = buffer.cursor();
            let character = match buffer.lines.get(cursor.line) {
                Some(line) => char_to_utf16(line, cursor.column),
                None => 0,
            };
            position = Position::new(cursor.line as u32, character);
            found = self.slot_for_path(path);
        }
        let slot = match found {
            Some(slot) => slot,
            None => {
                if intent.reports() {
                    self.report_missing(None);
                }
                return None;
            }
        };
        if !self.sessions[slot].server.as_ref().is_some_and(Server::ready) {
            if intent.reports() {
                self.report_missing(Some(slot));
            }
            return None;
        }
        self.cancel_intent(slot, intent);
        let version = self.workspace.doc_version(index);
        Some((slot, position, version))
    }

    fn report_missing(&mut self, slot: Option<usize>) {
        if self.pending.len() >= MAX_NOTICES {
            return;
        }
        let text = match slot {
            Some(slot) => match self.sessions[slot].server.as_ref() {
                Some(_) => NOT_READY.to_string(),
                None => self.sessions[slot].status.clone(),
            },
            None => NO_SERVER.to_string(),
        };
        self.pending.push(Notice::Failed(text));
    }

    fn cancel_intent(&mut self, slot: usize, intent: Intent) {
        let mut index = 0usize;
        while index < self.requests.len() {
            if self.requests[index].slot != slot || self.requests[index].intent != intent {
                index += 1;
                continue;
            }
            let id = self.requests.swap_remove(index).id;
            if let Some(server) = self.sessions[slot].server.as_mut() {
                server.cancel(id);
            }
        }
    }

    fn take(&mut self, slot: usize, id: u64) -> Option<Request> {
        let index = self
            .requests
            .iter()
            .position(|request| request.slot == slot && request.id == id)?;
        Some(self.requests.swap_remove(index))
    }

    fn stale(&self, request: &Request) -> bool {
        if !request.intent.versioned() {
            return false;
        }
        match open_index(&self.workspace, &request.path) {
            Some(index) => self.workspace.doc_version(index) != request.version,
            None => true,
        }
    }

    fn slot_for_path(&self, path: &Path) -> Option<usize> {
        let held = self
            .sessions
            .iter()
            .position(|session| session.docs.iter().any(|doc| doc == path));
        if held.is_some() {
            return held;
        }
        let (kind, root) = self.project.server_for(path)?;
        self.sessions
            .iter()
            .position(|session| session.kind == kind && session.root == root)
    }

    fn ensure_session(&mut self, path: &Path) -> Option<usize> {
        if let Some(slot) = self.slot_for_path(path) {
            return Some(slot);
        }
        let (kind, root) = self.project.server_for(path)?;
        let mut session =
            Session { kind, root, server: None, status: String::new(), docs: Vec::new() };
        match Server::start(kind, &session.root) {
            Ok(server) => {
                session.status.push_str(kind.label());
                session.status.push_str(": demarrage");
                session.server = Some(server);
            }
            Err(error) => {
                session.status.push_str(&error);
                session.status.push_str(", sans intelligence de langage");
            }
        }
        self.sessions.push(session);
        Some(self.sessions.len() - 1)
    }

    fn attach(&mut self, index: usize) {
        let Some(path) = self.workspace.tabs().get(index).and_then(|tab| tab.path.clone()) else {
            return;
        };
        let Some(slot) = self.ensure_session(&path) else {
            return;
        };
        if self.sessions[slot].docs.contains(&path) {
            return;
        }
        let mut text = std::mem::take(&mut self.text);
        document_text(self.workspace.buffer_at(index), &mut text);
        if let Some(server) = self.sessions[slot].server.as_mut() {
            server.open(&path, &text);
        }
        self.text = text;
        self.sessions[slot].docs.push(path);
    }

    fn detach(&mut self, path: &Path) {
        for session in self.sessions.iter_mut() {
            let Some(at) = session.docs.iter().position(|doc| doc == path) else {
                continue;
            };
            session.docs.swap_remove(at);
            if let Some(server) = session.server.as_mut() {
                server.close(path);
            }
        }
    }

    fn mark(&mut self) {
        let count = self.workspace.len();
        if self.saved.len() != count {
            self.saved.resize(count, false);
        }
        for index in 0..count {
            self.saved[index] = self.workspace.buffer_at(index).modified;
        }
    }

    fn announce_status(&mut self) {
        if !self.refresh_status() || self.pending.len() >= MAX_NOTICES {
            return;
        }
        let text = self.status.clone();
        self.pending.push(Notice::Status(text));
    }

    fn refresh_status(&mut self) -> bool {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        let slot = self
            .workspace
            .buffer()
            .path
            .as_deref()
            .and_then(|path| self.slot_for_path(path));
        match slot {
            Some(slot) => {
                let session = &self.sessions[slot];
                match session.server.as_ref() {
                    Some(server) => {
                        scratch.push_str(session.kind.label());
                        scratch.push_str(": ");
                        scratch.push_str(server.status());
                    }
                    None => scratch.push_str(&session.status),
                }
            }
            None => scratch.push_str(NO_SERVER),
        }
        let changed = scratch != self.status;
        if changed {
            self.status.clear();
            self.status.push_str(&scratch);
        }
        self.scratch = scratch;
        changed
    }

    fn convert_completion(&mut self, path: &Path, items: &mut [CompletionItem]) {
        let workspace = &self.workspace;
        let disk = &mut self.disk;
        for item in items.iter_mut() {
            if let Some((range, _)) = item.edit.as_mut() {
                convert_range(range, path, workspace, disk, true);
            }
        }
    }

    fn convert_locations(&mut self, locations: &mut Vec<Location>) {
        let workspace = &self.workspace;
        let disk = &mut self.disk;
        let mut raw = PathBuf::new();
        let mut full = PathBuf::new();
        for location in locations.iter_mut() {
            if location.path != raw {
                raw.clear();
                raw.push(&location.path);
                full = canonical(&location.path);
            }
            location.path.clear();
            location.path.push(&full);
            convert_range(&mut location.range, &full, workspace, disk, true);
        }
    }

    fn convert_symbols(&mut self, items: &mut [Symbol]) {
        let workspace = &self.workspace;
        let disk = &mut self.disk;
        let mut raw = PathBuf::new();
        let mut full = PathBuf::new();
        for item in items.iter_mut() {
            if item.path != raw {
                raw.clear();
                raw.push(&item.path);
                full = canonical(&item.path);
            }
            item.path.clear();
            item.path.push(&full);
            convert_range(&mut item.range, &full, workspace, disk, true);
            convert_range(&mut item.selection, &full, workspace, disk, true);
        }
    }

    fn locations_to_symbols(&mut self, locations: Vec<Location>) -> Vec<Symbol> {
        let mut out: Vec<Symbol> = Vec::with_capacity(locations.len());
        let workspace = &self.workspace;
        let disk = &mut self.disk;
        let root = self.project.root();
        let mut raw = PathBuf::new();
        let mut full = PathBuf::new();
        for location in locations {
            if location.path != raw {
                raw.clear();
                raw.push(&location.path);
                full = canonical(&location.path);
            }
            let mut range = location.range;
            convert_range(&mut range, &full, workspace, disk, true);
            let line = range.start.line as usize;
            let mut name = String::new();
            match open_index(workspace, &full) {
                Some(index) => {
                    if let Some(text) = workspace.buffer_at(index).lines.get(line) {
                        preview_into(text, &mut name);
                    }
                }
                None => {
                    if let Some(text) = disk.load(&full).line(line) {
                        preview_into(text, &mut name);
                    }
                }
            }
            if name.is_empty() {
                name.push_str("emplacement");
            }
            let mut container = String::new();
            let relative = full.strip_prefix(root).unwrap_or(&full);
            let _ = write!(container, "{}:{}", relative.display(), line + 1);
            out.push(Symbol {
                name,
                kind: 0,
                container,
                range,
                selection: range,
                path: full.clone(),
                depth: 0,
            });
        }
        out
    }
}

impl Drop for Ide {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn open_index(workspace: &Workspace, path: &Path) -> Option<usize> {
    workspace
        .tabs()
        .iter()
        .position(|tab| tab.path.as_deref() == Some(path))
}

fn document_text(buffer: &TextBuffer, out: &mut String) {
    out.clear();
    let mut total = buffer.lines.len();
    for line in buffer.lines.iter() {
        total += line.len();
    }
    out.reserve(total);
    for (index, line) in buffer.lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
}

fn shift(position: &mut Position, line: Option<&str>) {
    if let Some(line) = line {
        position.character = utf16_to_char(line, position.character) as u32;
    }
}

fn convert_range(
    range: &mut Range,
    path: &Path,
    workspace: &Workspace,
    disk: &mut DiskLines,
    prefer_buffer: bool,
) {
    if range.start.character == 0 && range.end.character == 0 {
        return;
    }
    let first = range.start.line as usize;
    let last = range.end.line as usize;
    if prefer_buffer
        && let Some(index) = open_index(workspace, path)
    {
        let buffer = workspace.buffer_at(index);
        shift(&mut range.start, buffer.lines.get(first).map(String::as_str));
        shift(&mut range.end, buffer.lines.get(last).map(String::as_str));
        return;
    }
    let lines = disk.load(path);
    if lines.ascii {
        return;
    }
    shift(&mut range.start, lines.line(first));
    shift(&mut range.end, lines.line(last));
}

fn cursor_at(buffer: &TextBuffer, position: Position) -> Cursor {
    let last = buffer.line_count() - 1;
    let line = (position.line as usize).min(last);
    let column = match buffer.lines.get(line) {
        Some(text) => utf16_to_char(text, position.character),
        None => 0,
    };
    Cursor { line, column: column.min(buffer.line_chars(line)) }
}

fn preview_into(text: &str, out: &mut String) {
    let mut space = false;
    let mut count = 0usize;
    for character in text.trim().chars() {
        if count >= PREVIEW_CHARS {
            out.push_str("...");
            return;
        }
        if character.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
            count += 1;
        }
        space = false;
        out.push(character);
        count += 1;
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

fn copy_into(target: &mut String, source: &str) {
    target.clear();
    target.push_str(source);
}

fn assign_one(slot: &mut Diagnostic, item: &Diagnostic) {
    slot.range = item.range;
    slot.severity = item.severity;
    copy_into(&mut slot.message, &item.message);
    copy_into(&mut slot.source, &item.source);
    copy_into(&mut slot.code, &item.code);
}

fn assign_slot(list: &mut Vec<Diagnostic>, at: usize, item: &Diagnostic) {
    if at < list.len() {
        assign_one(&mut list[at], item);
    } else {
        list.push(item.clone());
    }
}

fn entry_of(files: &mut Vec<DiagFile>, path: &Path) -> usize {
    match files.binary_search_by(|entry| entry.path.as_path().cmp(path)) {
        Ok(index) => index,
        Err(index) => {
            files.insert(
                index,
                DiagFile {
                    path: path.to_path_buf(),
                    lsp: Vec::new(),
                    task: Vec::new(),
                    merged: Vec::new(),
                    stale: false,
                    notify: false,
                },
            );
            index
        }
    }
}

fn duplicate(left: &Diagnostic, right: &Diagnostic) -> bool {
    if left.range.start.line != right.range.start.line
        || left.range.start.character != right.range.start.character
        || left.severity != right.severity
    {
        return false;
    }
    if left.message == right.message {
        return true;
    }
    !left.code.is_empty() && left.code == right.code
}

fn merge_entry(files: &mut [DiagFile], index: usize) {
    let entry = &mut files[index];
    let merged = &mut entry.merged;
    let mut at = 0usize;
    for item in entry.lsp.iter() {
        assign_slot(merged, at, item);
        at += 1;
    }
    for item in entry.task.iter() {
        assign_slot(merged, at, item);
        at += 1;
    }
    merged.truncate(at);
    merged.sort_by(|left, right| {
        left.range
            .start
            .line
            .cmp(&right.range.start.line)
            .then(left.range.start.character.cmp(&right.range.start.character))
            .then(severity_rank(left.severity).cmp(&severity_rank(right.severity)))
    });
    merged.dedup_by(|left, right| duplicate(left, right));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextBuffer;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let directory = std::env::temp_dir().join(format!("text3d-ide-{name}-{stamp}"));
        std::fs::create_dir_all(&directory).expect("dossier temporaire");
        std::fs::canonicalize(&directory).unwrap_or(directory)
    }

    fn plain_ide(root: &Path) -> Ide {
        let project = Project::open(root);
        let workspace = Workspace::new(TextBuffer::from_str("", None));
        Ide::new(project, workspace)
    }

    fn diagnostic(line: u32, start: u32, end: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, start), Position::new(line, end)),
            severity,
            message: message.to_string(),
            source: "essai".to_string(),
            code: String::new(),
        }
    }

    #[test]
    fn sans_serveur_l_ide_reste_utilisable() {
        let root = temporary("sans-serveur");
        let file = root.join("notes.txt");
        std::fs::write(&file, "premiere ligne\ndeuxieme ligne\n").expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");
        assert!(!ide.server_ready());
        assert_eq!(ide.server_status(), NO_SERVER);

        let mut notices = Vec::new();
        ide.poll(&mut notices);
        assert!(notices.iter().any(|notice| matches!(notice, Notice::Status(_))));

        ide.workspace_mut().buffer_mut().set_cursor(Cursor { line: 0, column: 0 }, false);
        ide.workspace_mut().buffer_mut().insert_str("bonjour ");
        ide.notify_edit();
        assert_eq!(ide.workspace().buffer().lines[0], "bonjour premiere ligne");

        notices.clear();
        ide.ask_completion(None);
        ide.ask_definition();
        ide.ask_references();
        ide.ask_format();
        ide.ask_rename("autre");
        ide.ask_workspace_symbols("x");
        ide.poll(&mut notices);
        let refus = notices
            .iter()
            .filter(|notice| matches!(notice, Notice::Failed(_)))
            .count();
        assert!(refus >= 4, "les demandes doivent etre refusees proprement: {refus}");
        assert!(ide.workspace_mut().buffer_mut().undo());
        ide.notify_edit();
        assert_eq!(ide.workspace().buffer().lines[0], "premiere ligne");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn positions_lsp_converties_en_colonnes_de_caracteres() {
        let root = temporary("utf16");
        let file = root.join("accents.txt");
        let source = "let x = \"caf\u{e9} \u{1f680}\"; erreur\n";
        std::fs::write(&file, source).expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");

        let line = ide.workspace().buffer().lines[0].clone();
        let target = line.chars().count() - 6;
        let units = char_to_utf16(&line, target);
        assert_ne!(units as usize, target, "l emoji doit decaler l unite utf-16");

        ide.store_lsp(
            file.clone(),
            vec![diagnostic(0, units, units + 6, Severity::Error, "mot inconnu")],
        );
        let mut notices = Vec::new();
        ide.poll(&mut notices);

        let stored = ide.diagnostics_for(&file);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].range.start.character as usize, target);
        let word: String = line.chars().skip(target).take(6).collect();
        assert_eq!(word, "erreur");
        assert!(notices.iter().any(|notice| matches!(notice, Notice::Diagnostics(_))));
        assert_eq!(ide.diagnostic_counts(), (1, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnostics_par_ligne_en_recherche_binaire() {
        let root = temporary("par-ligne");
        let file = root.join("code.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne\n").expect("fichier");
        let mut ide = plain_ide(&root);
        let items = vec![
            diagnostic(4, 0, 1, Severity::Warning, "quatre"),
            diagnostic(1, 0, 1, Severity::Error, "un"),
            diagnostic(1, 2, 3, Severity::Warning, "un bis"),
            diagnostic(3, 0, 1, Severity::Error, "trois"),
        ];
        ide.store_lsp(file.clone(), items);
        let mut notices = Vec::new();
        ide.poll(&mut notices);

        assert_eq!(ide.diagnostics_for(&file).len(), 4);
        assert_eq!(ide.diagnostics_on_line(&file, 0).len(), 0);
        assert_eq!(ide.diagnostics_on_line(&file, 1).len(), 2);
        assert_eq!(ide.diagnostics_on_line(&file, 1)[0].message, "un");
        assert_eq!(ide.diagnostics_on_line(&file, 3).len(), 1);
        assert_eq!(ide.diagnostics_on_line(&file, 4)[0].message, "quatre");
        assert_eq!(ide.diagnostics_on_line(&file, 99).len(), 0);
        assert_eq!(ide.diagnostic_counts(), (2, 2));
        assert_eq!(ide.all_diagnostics().len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnostics_du_compilateur_fusionnes_et_dedoublonnes() {
        let root = temporary("fusion");
        let file = root.join("code.txt");
        std::fs::write(&file, "une ligne\nune autre\n").expect("fichier");
        let mut ide = plain_ide(&root);
        let mut premier = diagnostic(0, 4, 9, Severity::Error, "identique");
        premier.code.push_str("E0425");
        ide.store_lsp(file.clone(), vec![premier]);

        let index = entry_of(&mut ide.diagnostics, &file);
        let mut second = diagnostic(0, 4, 9, Severity::Error, "rendu du compilateur");
        second.code.push_str("E0425");
        ide.diagnostics[index].task.push(second);
        ide.diagnostics[index].task.push(diagnostic(1, 0, 3, Severity::Warning, "autre"));
        ide.diagnostics[index].stale = true;

        let mut notices = Vec::new();
        ide.poll(&mut notices);
        let merged = ide.diagnostics_for(&file);
        assert_eq!(merged.len(), 2, "le doublon de code doit disparaitre");
        assert_eq!(merged[0].message, "identique");
        assert_eq!(merged[1].message, "autre");
        assert_eq!(ide.diagnostic_counts(), (1, 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn editions_appliquees_en_ordre_decroissant_puis_annulees() {
        let root = temporary("editions");
        let file = root.join("texte.txt");
        let source = "alpha beta gamma\nseconde ligne\n";
        std::fs::write(&file, source).expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");
        let avant = ide.workspace().buffer().lines.clone();

        let edits = vec![(
            file.clone(),
            vec![
                TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                    text: "un".to_string(),
                },
                TextEdit {
                    range: Range::new(Position::new(0, 11), Position::new(0, 16)),
                    text: "trois".to_string(),
                },
                TextEdit {
                    range: Range::new(Position::new(1, 0), Position::new(1, 7)),
                    text: "deuxieme".to_string(),
                },
            ],
        )];
        let (files, count) = ide.apply_edits(&edits).expect("editions");
        assert_eq!(files, 1);
        assert_eq!(count, 3);
        assert_eq!(ide.workspace().buffer().lines[0], "un beta trois");
        assert_eq!(ide.workspace().buffer().lines[1], "deuxieme ligne");

        for _ in 0..count {
            assert!(ide.workspace_mut().buffer_mut().undo());
        }
        assert_eq!(ide.workspace().buffer().lines, avant);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn un_meme_fichier_cite_deux_fois_reste_coherent() {
        let root = temporary("doublon");
        let file = root.join("texte.txt");
        std::fs::write(&file, "alpha beta gamma\n").expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");
        let edits = vec![
            (
                file.clone(),
                vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                    text: "un".to_string(),
                }],
            ),
            (
                file.clone(),
                vec![TextEdit {
                    range: Range::new(Position::new(0, 11), Position::new(0, 16)),
                    text: "trois".to_string(),
                }],
            ),
        ];
        let (files, count) = ide.apply_edits(&edits).expect("editions");
        assert_eq!(files, 1, "un seul fichier touche");
        assert_eq!(count, 2);
        assert_eq!(ide.workspace().buffer().lines[0], "un beta trois");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reponse_pour_une_version_perimee_est_jetee() {
        let root = temporary("version");
        let file = root.join("code.txt");
        std::fs::write(&file, "abc\n").expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");
        let index = ide.workspace().active();
        let version = ide.workspace().doc_version(index);
        let request = Request {
            slot: 0,
            id: 1,
            intent: Intent::Completion,
            path: file.clone(),
            version,
        };
        assert!(!ide.stale(&request));
        ide.workspace_mut().buffer_mut().insert_char('z');
        ide.notify_edit();
        assert!(ide.stale(&request), "une frappe doit perimer la reponse");

        let absent = Request {
            slot: 0,
            id: 2,
            intent: Intent::Completion,
            path: root.join("jamais-ouvert.txt"),
            version: 1,
        };
        assert!(ide.stale(&absent));
        let global = Request {
            slot: 0,
            id: 3,
            intent: Intent::WorkspaceSymbols,
            path: PathBuf::new(),
            version: 0,
        };
        assert!(!ide.stale(&global));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fermeture_refusee_puis_forcee() {
        let root = temporary("fermeture");
        let file = root.join("un.txt");
        let autre = root.join("deux.txt");
        std::fs::write(&file, "un\n").expect("fichier");
        std::fs::write(&autre, "deux\n").expect("fichier");
        let mut ide = plain_ide(&root);
        ide.open_file(&file, None).expect("ouverture");
        ide.open_file(&autre, None).expect("ouverture");
        assert_eq!(ide.workspace().len(), 2);
        ide.workspace_mut().buffer_mut().insert_char('x');
        ide.notify_edit();
        let active = ide.workspace().active();
        assert!(ide.close_tab(active, false).is_err());
        assert!(ide.close_tab(active, true).is_ok());
        assert_eq!(ide.workspace().len(), 1);
        assert!(ide.close_tab(9, false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tache_sans_projet_valide_echoue_proprement() {
        let root = temporary("tache");
        std::fs::write(root.join("note.txt"), "rien\n").expect("fichier");
        let mut ide = plain_ide(&root);
        let outcome = ide.run_task(Task::Check);
        assert!(outcome.is_err(), "un dossier sans manifeste ne lance rien");
        ide.stop_task();
        let mut notices = Vec::new();
        ide.poll(&mut notices);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recherche_dans_le_projet_puis_apercu() {
        let root = temporary("recherche");
        std::fs::write(root.join("a.txt"), "mot cherche ici\n").expect("fichier");
        std::fs::write(root.join("b.txt"), "rien du tout\n").expect("fichier");
        let mut ide = plain_ide(&root);
        let mut notices = Vec::new();
        for _ in 0..200 {
            ide.poll(&mut notices);
            if ide.project().indexed().1 && ide.project().files().len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        ide.start_search("cherche", false, false);
        for _ in 0..400 {
            ide.poll(&mut notices);
            if !ide.search().running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(ide.search().hits().len(), 1);
        assert_eq!(ide.search().hits()[0].line, 0);
        assert!(notices.iter().any(|notice| matches!(notice, Notice::Status(_))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apercu_de_ligne_condense_les_espaces() {
        let mut out = String::new();
        preview_into("    let   x =    1;   ", &mut out);
        assert_eq!(out, "let x = 1;");
        out.clear();
        let long = "a".repeat(400);
        preview_into(&long, &mut out);
        assert!(out.ends_with("..."));
        assert!(out.chars().count() <= PREVIEW_CHARS + 3);
    }

    #[test]
    fn texte_du_document_recolle_les_lignes() {
        let buffer = TextBuffer::from_str("une\ndeux\ntrois", None);
        let mut out = String::from("residu");
        document_text(&buffer, &mut out);
        assert_eq!(out, "une\ndeux\ntrois");
    }
}
