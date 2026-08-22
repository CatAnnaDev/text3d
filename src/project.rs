use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use crate::json::Json;
use crate::lsp::session::ServerKind;

const MAX_FILES: usize = 200_000;
const MAX_DEPTH: usize = 12;
const MAX_ASCENT: usize = 24;
const BATCH: usize = 512;

const EXCLUDED_NAMES: [&str; 12] = [
    ".git",
    "target",
    "bin",
    "obj",
    "node_modules",
    ".vs",
    ".vscode",
    ".idea",
    ".godot",
    "Library",
    "Temp",
    ".DS_Store",
];

const SOURCE_EXTENSIONS: [&str; 16] = [
    "rs", "cs", "toml", "csproj", "sln", "json", "md", "wgsl", "glsl", "shader", "xml", "yml",
    "yaml", "txt", "sh", "py",
];

const PROJECT_EXTENSIONS: [&str; 3] = ["csproj", "fsproj", "vbproj"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Rust,
    CSharp,
    Mixed,
    Plain,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Rust => "rust",
            Kind::CSharp => "c#",
            Kind::Mixed => "mixte",
            Kind::Plain => "texte",
        }
    }
}

pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    pub children: usize,
}

pub struct Target {
    pub name: String,
    pub kind: String,
    pub manifest: PathBuf,
}

struct Node {
    path: PathBuf,
    name: String,
    is_dir: bool,
    expanded: bool,
    loaded: bool,
    children: Vec<usize>,
}

struct Rule {
    pattern: String,
    anchored: bool,
    directory_only: bool,
    plain: bool,
}

struct Excluder {
    rules: Vec<Rule>,
    anchored: bool,
}

impl Excluder {
    fn load(root: &Path) -> Excluder {
        let mut rules = Vec::new();
        let mut anchored = false;
        if let Ok(text) = fs::read_to_string(root.join(".gitignore")) {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                    continue;
                }
                let mut pattern = trimmed;
                let directory_only = pattern.ends_with('/');
                while let Some(stripped) = pattern.strip_suffix('/') {
                    pattern = stripped;
                }
                let rooted = pattern.starts_with('/');
                while let Some(stripped) = pattern.strip_prefix('/') {
                    pattern = stripped;
                }
                if pattern.is_empty() {
                    continue;
                }
                let is_anchored = rooted || pattern.contains('/');
                anchored |= is_anchored;
                let plain = !pattern.contains('*') && !pattern.contains('?');
                rules.push(Rule {
                    pattern: pattern.to_string(),
                    anchored: is_anchored,
                    directory_only,
                    plain,
                });
            }
        }
        Excluder { rules, anchored }
    }

    fn excluded(
        &self,
        path: &Path,
        name: &str,
        is_dir: bool,
        root: &Path,
        scratch: &mut String,
    ) -> bool {
        for excluded in EXCLUDED_NAMES {
            if name == excluded {
                return true;
            }
        }
        if self.rules.is_empty() {
            return false;
        }
        if self.anchored {
            scratch.clear();
            if let Ok(relative) = path.strip_prefix(root) {
                for component in relative.components() {
                    if !scratch.is_empty() {
                        scratch.push('/');
                    }
                    scratch.push_str(&component.as_os_str().to_string_lossy());
                }
            }
        }
        for rule in &self.rules {
            if rule.directory_only && !is_dir {
                continue;
            }
            let target = if rule.anchored {
                scratch.as_str()
            } else {
                name
            };
            if target.is_empty() {
                continue;
            }
            let hit = if rule.plain {
                target == rule.pattern
            } else {
                wildcard(&rule.pattern, target)
            };
            if hit {
                return true;
            }
        }
        false
    }
}

fn wildcard(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star = usize::MAX;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            pi += 1;
            mark = ti;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn compare_names(left: &str, right: &str) -> Ordering {
    let mut a = left.chars().flat_map(char::to_lowercase);
    let mut b = right.chars().flat_map(char::to_lowercase);
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

fn compare_paths(left: &Path, right: &Path) -> Ordering {
    match (left.file_name(), right.file_name()) {
        (Some(a), Some(b)) => compare_names(&a.to_string_lossy(), &b.to_string_lossy()),
        _ => left.cmp(right),
    }
}

fn has_extension(name: &str, extension: &str) -> bool {
    match name.len().checked_sub(extension.len() + 1) {
        Some(split) => {
            name.as_bytes()[split] == b'.' && name[split + 1..].eq_ignore_ascii_case(extension)
        }
        None => false,
    }
}

fn is_project_file(name: &str) -> bool {
    PROJECT_EXTENSIONS
        .iter()
        .any(|extension| has_extension(name, extension))
}

fn is_source(path: &Path) -> bool {
    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) => SOURCE_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known)),
        None => false,
    }
}

fn normalize(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn directory_markers(directory: &Path) -> (bool, bool) {
    let mut solution = false;
    let mut project = false;
    if let Ok(reader) = fs::read_dir(directory) {
        for item in reader.flatten() {
            let raw = item.file_name();
            let name = raw.to_string_lossy();
            if has_extension(&name, "sln") {
                solution = true;
            } else if is_project_file(&name) {
                project = true;
            }
        }
    }
    (solution, project)
}

fn has_marker(directory: &Path) -> bool {
    if directory.join("Cargo.toml").is_file() || directory.join(".git").exists() {
        return true;
    }
    let (solution, project) = directory_markers(directory);
    solution || project
}

fn resolve_root(path: &Path) -> PathBuf {
    let canonical = normalize(path.to_path_buf());
    if fs::metadata(&canonical)
        .map(|data| data.is_dir())
        .unwrap_or(false)
    {
        return canonical;
    }
    let start = match canonical.parent() {
        Some(parent) => parent.to_path_buf(),
        None => return canonical,
    };
    let mut current = start.as_path();
    let mut levels = 0usize;
    loop {
        if has_marker(current) {
            return current.to_path_buf();
        }
        levels += 1;
        if levels > MAX_ASCENT {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    start
}

fn scan_markers(directory: &Path, rust: &mut bool, csharp: &mut bool) {
    let Ok(reader) = fs::read_dir(directory) else {
        return;
    };
    for item in reader.flatten() {
        let raw = item.file_name();
        let name = raw.to_string_lossy();
        if name == "Cargo.toml" {
            *rust = true;
        } else if has_extension(&name, "sln") || is_project_file(&name) {
            *csharp = true;
        }
    }
}

fn detect_kind(root: &Path, excluder: &Excluder) -> Kind {
    let mut rust = false;
    let mut csharp = false;
    scan_markers(root, &mut rust, &mut csharp);
    if !(rust && csharp)
        && let Ok(reader) = fs::read_dir(root)
    {
        let mut scratch = String::new();
        for item in reader.flatten() {
            if !item.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let raw = item.file_name();
            let name = raw.to_string_lossy();
            let path = item.path();
            if excluder.excluded(&path, &name, true, root, &mut scratch) {
                continue;
            }
            scan_markers(&path, &mut rust, &mut csharp);
            if rust && csharp {
                break;
            }
        }
    }
    match (rust, csharp) {
        (true, true) => Kind::Mixed,
        (true, false) => Kind::Rust,
        (false, true) => Kind::CSharp,
        (false, false) => Kind::Plain,
    }
}

fn manifest_declares_workspace(manifest: &Path) -> bool {
    let Ok(text) = fs::read_to_string(manifest) else {
        return false;
    };
    text.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "[workspace]" || trimmed.starts_with("[workspace.")
    })
}

fn rust_root(directory: &Path) -> Option<PathBuf> {
    let mut nearest: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut current = directory;
    let mut levels = 0usize;
    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            if nearest.is_none() {
                nearest = Some(current.to_path_buf());
            }
            if manifest_declares_workspace(&manifest) {
                workspace = Some(current.to_path_buf());
            }
        }
        levels += 1;
        if levels > MAX_ASCENT || current.join(".git").exists() {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    workspace.or(nearest)
}

fn csharp_root(directory: &Path) -> Option<PathBuf> {
    let mut project: Option<PathBuf> = None;
    let mut current = directory;
    let mut levels = 0usize;
    loop {
        let (solution, has_project) = directory_markers(current);
        if solution {
            return Some(current.to_path_buf());
        }
        if has_project && project.is_none() {
            project = Some(current.to_path_buf());
        }
        levels += 1;
        if levels > MAX_ASCENT || current.join(".git").exists() {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    project
}

fn tag_value<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(close)?;
    let value = rest[..end].trim();
    if value.is_empty() { None } else { Some(value) }
}

fn target_framework(text: &str) -> Option<&str> {
    tag_value(text, "<TargetFramework>", "</TargetFramework>")
        .or_else(|| tag_value(text, "<TargetFrameworks>", "</TargetFrameworks>"))
}

fn read_into(path: &Path, out: &mut String) -> Result<(), String> {
    out.clear();
    let mut file = fs::File::open(path).map_err(|error| format!("lecture impossible: {error}"))?;
    file.read_to_string(out)
        .map_err(|error| format!("lecture impossible: {error}"))?;
    Ok(())
}

fn solution_projects(solution: &Path, out: &mut Vec<PathBuf>) {
    let Some(directory) = solution.parent() else {
        return;
    };
    let Ok(text) = fs::read_to_string(solution) else {
        return;
    };
    let mut relative = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("Project(") {
            continue;
        }
        let Some(raw) = trimmed.split('"').nth(5) else {
            continue;
        };
        if !is_project_file(raw) {
            continue;
        }
        relative.clear();
        for piece in raw.split('\\') {
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(piece);
        }
        let candidate = normalize(directory.join(&relative));
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
}

fn collect_csharp_files(
    root: &Path,
    excluder: &Excluder,
    projects: &mut Vec<PathBuf>,
    solutions: &mut Vec<PathBuf>,
) {
    let mut stack: Vec<(PathBuf, usize)> = Vec::with_capacity(32);
    stack.push((root.to_path_buf(), 0));
    let mut scratch = String::new();
    while let Some((directory, depth)) = stack.pop() {
        let Ok(reader) = fs::read_dir(&directory) else {
            continue;
        };
        for item in reader.flatten() {
            let raw = item.file_name();
            let name = raw.to_string_lossy();
            let is_dir = item.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let path = item.path();
            if excluder.excluded(&path, &name, is_dir, root, &mut scratch) {
                continue;
            }
            if is_dir {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if is_project_file(&name) {
                projects.push(path);
            } else if has_extension(&name, "sln") {
                solutions.push(path);
            }
        }
    }
}

fn cargo_program() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let bundled = PathBuf::from(home).join(".cargo/bin/cargo");
        if bundled.is_file() {
            return bundled;
        }
    }
    PathBuf::from("cargo")
}

fn cargo_targets(root: &Path) -> Result<Vec<Target>, String> {
    let output = Command::new(cargo_program())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cargo indisponible: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let first = detail
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("cause inconnue");
        return Err(format!("cargo metadata a echoue: {first}"));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "sortie de cargo metadata illisible".to_string())?;
    let document = Json::parse(&text)?;
    let packages = document
        .get("packages")
        .and_then(Json::as_array)
        .ok_or_else(|| "cargo metadata sans liste de paquets".to_string())?;
    let mut targets = Vec::new();
    for package in packages {
        let manifest = PathBuf::from(
            package
                .get("manifest_path")
                .and_then(Json::as_str)
                .unwrap_or_default(),
        );
        let Some(list) = package.get("targets").and_then(Json::as_array) else {
            continue;
        };
        targets.reserve(list.len());
        for item in list {
            let Some(name) = item.get("name").and_then(Json::as_str) else {
                continue;
            };
            let mut kind = String::new();
            if let Some(kinds) = item.get("kind").and_then(Json::as_array) {
                for entry in kinds {
                    let Some(value) = entry.as_str() else {
                        continue;
                    };
                    if !kind.is_empty() {
                        kind.push('+');
                    }
                    kind.push_str(value);
                }
            }
            if kind.is_empty() {
                kind.push_str("inconnu");
            }
            targets.push(Target {
                name: name.to_string(),
                kind,
                manifest: manifest.clone(),
            });
        }
    }
    Ok(targets)
}

fn csharp_targets(root: &Path, excluder: &Excluder) -> Result<Vec<Target>, String> {
    let mut projects = Vec::new();
    let mut solutions = Vec::new();
    collect_csharp_files(root, excluder, &mut projects, &mut solutions);
    for solution in &solutions {
        solution_projects(solution, &mut projects);
    }
    projects.sort();
    projects.dedup();
    if projects.is_empty() {
        return Err("aucun projet c# trouve".to_string());
    }
    let mut targets = Vec::with_capacity(projects.len());
    let mut text = String::new();
    for manifest in projects {
        let name = manifest
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = match read_into(&manifest, &mut text) {
            Ok(()) => target_framework(&text)
                .map(str::to_string)
                .unwrap_or_else(|| "csproj".to_string()),
            Err(_) => "illisible".to_string(),
        };
        targets.push(Target {
            name,
            kind,
            manifest,
        });
    }
    Ok(targets)
}

fn collect_targets(
    root: PathBuf,
    kind: Kind,
    excluder: Arc<Excluder>,
) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();
    let mut problem = String::new();
    if matches!(kind, Kind::Rust | Kind::Mixed) {
        match cargo_targets(&root) {
            Ok(mut list) => targets.append(&mut list),
            Err(message) => problem.push_str(&message),
        }
    }
    if matches!(kind, Kind::CSharp | Kind::Mixed) {
        match csharp_targets(&root, &excluder) {
            Ok(mut list) => targets.append(&mut list),
            Err(message) => {
                if !problem.is_empty() {
                    problem.push_str(" ; ");
                }
                problem.push_str(&message);
            }
        }
    }
    if targets.is_empty() && !problem.is_empty() {
        return Err(problem);
    }
    Ok(targets)
}

fn walk_sources(
    root: PathBuf,
    excluder: Arc<Excluder>,
    cancel: Arc<AtomicBool>,
    sender: Sender<Vec<PathBuf>>,
) {
    let mut stack: Vec<(PathBuf, usize)> = Vec::with_capacity(64);
    stack.push((root.clone(), 0));
    let mut batch: Vec<PathBuf> = Vec::with_capacity(BATCH);
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut scratch = String::new();
    let mut total = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if cancel.load(AtomicOrdering::Relaxed) {
            return;
        }
        let Ok(reader) = fs::read_dir(&directory) else {
            continue;
        };
        for item in reader.flatten() {
            let raw = item.file_name();
            let name = raw.to_string_lossy();
            let is_dir = item.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let path = item.path();
            if excluder.excluded(&path, &name, is_dir, &root, &mut scratch) {
                continue;
            }
            if is_dir {
                if depth < MAX_DEPTH {
                    directories.push(path);
                }
            } else if is_source(&path) {
                files.push(path);
            }
        }
        files.sort_by(|left, right| compare_paths(left, right));
        directories.sort_by(|left, right| compare_paths(left, right));
        for path in files.drain(..) {
            batch.push(path);
            total += 1;
            if total >= MAX_FILES {
                let _ = sender.send(batch);
                return;
            }
            if batch.len() >= BATCH {
                let chunk = std::mem::replace(&mut batch, Vec::with_capacity(BATCH));
                if sender.send(chunk).is_err() {
                    return;
                }
            }
        }
        for path in directories.drain(..).rev() {
            stack.push((path, depth + 1));
        }
    }
    if !batch.is_empty() {
        let _ = sender.send(batch);
    }
}

pub struct Project {
    root: PathBuf,
    label: String,
    kind: Kind,
    nodes: Vec<Node>,
    entries: Vec<Entry>,
    entry_nodes: Vec<usize>,
    stack: Vec<(usize, usize)>,
    excluder: Arc<Excluder>,
    files: Vec<PathBuf>,
    scanning: bool,
    truncated: bool,
    scan_rx: Option<Receiver<Vec<PathBuf>>>,
    scan_cancel: Arc<AtomicBool>,
    scan_handle: Option<JoinHandle<()>>,
    targets: Vec<Target>,
    targets_error: Option<String>,
    targets_pending: bool,
    targets_rx: Option<Receiver<Result<Vec<Target>, String>>>,
    rust_roots: RefCell<HashMap<PathBuf, Option<PathBuf>>>,
    csharp_roots: RefCell<HashMap<PathBuf, Option<PathBuf>>>,
}

impl Project {
    pub fn open(path: &Path) -> Project {
        let root = resolve_root(path);
        let label = root
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let excluder = Arc::new(Excluder::load(&root));
        let kind = detect_kind(&root, &excluder);
        let mut project = Project {
            root,
            label,
            kind,
            nodes: Vec::new(),
            entries: Vec::new(),
            entry_nodes: Vec::new(),
            stack: Vec::new(),
            excluder,
            files: Vec::new(),
            scanning: false,
            truncated: false,
            scan_rx: None,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            scan_handle: None,
            targets: Vec::new(),
            targets_error: None,
            targets_pending: false,
            targets_rx: None,
            rust_roots: RefCell::new(HashMap::new()),
            csharp_roots: RefCell::new(HashMap::new()),
        };
        project.build_tree();
        project.start_scan();
        project.start_targets();
        project
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn toggle(&mut self, index: usize) {
        let Some(&node) = self.entry_nodes.get(index) else {
            return;
        };
        if !self.nodes[node].is_dir {
            return;
        }
        if self.nodes[node].expanded {
            self.nodes[node].expanded = false;
        } else {
            self.load_children(node);
            self.nodes[node].expanded = true;
        }
        self.rebuild();
    }

    pub fn expand_to(&mut self, path: &Path) {
        let target = normalize(path.to_path_buf());
        let Ok(relative) = target.strip_prefix(&self.root) else {
            return;
        };
        self.load_children(0);
        let mut changed = !self.nodes[0].expanded;
        self.nodes[0].expanded = true;
        let mut node = 0usize;
        for component in relative.components() {
            let name = component.as_os_str().to_string_lossy();
            let found = self.nodes[node]
                .children
                .iter()
                .copied()
                .find(|&child| self.nodes[child].name == name);
            let Some(child) = found else {
                break;
            };
            node = child;
            if !self.nodes[node].is_dir {
                break;
            }
            self.load_children(node);
            if !self.nodes[node].expanded {
                self.nodes[node].expanded = true;
                changed = true;
            }
        }
        if changed {
            self.rebuild();
        }
    }

    pub fn poll(&mut self) -> bool {
        if let Some(receiver) = &self.scan_rx {
            loop {
                match receiver.try_recv() {
                    Ok(batch) => {
                        if self.files.is_empty() {
                            self.files.reserve(batch.len() * 4);
                        }
                        self.files.extend(batch);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.scanning = false;
                        break;
                    }
                }
            }
        }
        if !self.scanning && self.scan_rx.is_some() {
            self.scan_rx = None;
            if let Some(handle) = self.scan_handle.take() {
                let _ = handle.join();
            }
            self.truncated = self.files.len() >= MAX_FILES;
        }
        if let Some(receiver) = &self.targets_rx {
            match receiver.try_recv() {
                Ok(Ok(list)) => {
                    self.targets = list;
                    self.targets_pending = false;
                }
                Ok(Err(message)) => {
                    self.targets_error = Some(message);
                    self.targets_pending = false;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    if self.targets_pending {
                        self.targets_error =
                            Some("l analyse des cibles s est interrompue".to_string());
                    }
                    self.targets_pending = false;
                }
            }
        }
        if !self.targets_pending {
            self.targets_rx = None;
        }
        self.scanning || self.targets_pending
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn indexed(&self) -> (usize, bool) {
        (self.files.len(), !self.scanning)
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    pub fn targets_error(&self) -> Option<&str> {
        self.targets_error.as_deref()
    }

    pub fn server_for(&self, path: &Path) -> Option<(ServerKind, PathBuf)> {
        let extension = path.extension()?.to_str()?;
        let directory = path.parent()?;
        if extension.eq_ignore_ascii_case("rs") {
            let cached = self.rust_roots.borrow().get(directory).cloned();
            if let Some(found) = cached {
                return found.map(|root| (ServerKind::Rust, root));
            }
            let resolved = rust_root(directory);
            self.rust_roots
                .borrow_mut()
                .insert(directory.to_path_buf(), resolved.clone());
            resolved.map(|root| (ServerKind::Rust, root))
        } else if extension.eq_ignore_ascii_case("cs") || extension.eq_ignore_ascii_case("csx") {
            let cached = self.csharp_roots.borrow().get(directory).cloned();
            if let Some(found) = cached {
                return found.map(|root| (ServerKind::CSharp, root));
            }
            let resolved = csharp_root(directory);
            self.csharp_roots
                .borrow_mut()
                .insert(directory.to_path_buf(), resolved.clone());
            resolved.map(|root| (ServerKind::CSharp, root))
        } else {
            None
        }
    }

    pub fn refresh(&mut self) {
        let mut opened: HashSet<PathBuf> = HashSet::with_capacity(self.nodes.len() / 4 + 1);
        for node in &self.nodes {
            if node.is_dir && node.expanded {
                opened.insert(node.path.clone());
            }
        }
        self.excluder = Arc::new(Excluder::load(&self.root));
        self.kind = detect_kind(&self.root, &self.excluder);
        self.rust_roots.borrow_mut().clear();
        self.csharp_roots.borrow_mut().clear();
        self.build_tree();
        let mut index = 0usize;
        while index < self.nodes.len() {
            if self.nodes[index].is_dir && opened.contains(&self.nodes[index].path) {
                self.load_children(index);
                self.nodes[index].expanded = true;
            }
            index += 1;
        }
        self.rebuild();
        self.start_scan();
        self.start_targets();
    }

    fn build_tree(&mut self) {
        self.nodes.clear();
        let name = self
            .root
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned());
        self.nodes.push(Node {
            path: self.root.clone(),
            name,
            is_dir: true,
            expanded: true,
            loaded: false,
            children: Vec::new(),
        });
        self.load_children(0);
        self.rebuild();
    }

    fn load_children(&mut self, node: usize) {
        if self.nodes[node].loaded {
            return;
        }
        self.nodes[node].loaded = true;
        let directory = self.nodes[node].path.clone();
        let mut buffer: Vec<Node> = Vec::new();
        let mut scratch = String::new();
        if let Ok(reader) = fs::read_dir(&directory) {
            for item in reader.flatten() {
                let raw = item.file_name();
                let name = raw.to_string_lossy();
                let is_dir = item.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                let path = item.path();
                if self
                    .excluder
                    .excluded(&path, &name, is_dir, &self.root, &mut scratch)
                {
                    continue;
                }
                buffer.push(Node {
                    path,
                    name: name.into_owned(),
                    is_dir,
                    expanded: false,
                    loaded: false,
                    children: Vec::new(),
                });
            }
        }
        buffer.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| compare_names(&left.name, &right.name))
        });
        let start = self.nodes.len();
        let count = buffer.len();
        self.nodes.reserve(count);
        self.nodes.append(&mut buffer);
        let mut children = Vec::with_capacity(count);
        for offset in 0..count {
            children.push(start + offset);
        }
        self.nodes[node].children = children;
    }

    fn rebuild(&mut self) {
        self.entries.clear();
        self.entry_nodes.clear();
        let mut stack = std::mem::take(&mut self.stack);
        stack.clear();
        stack.push((0, 0));
        while let Some((node, depth)) = stack.pop() {
            let current = &self.nodes[node];
            self.entries.push(Entry {
                path: current.path.clone(),
                name: current.name.clone(),
                is_dir: current.is_dir,
                depth,
                expanded: current.expanded,
                children: current.children.len(),
            });
            self.entry_nodes.push(node);
            if current.is_dir && current.expanded {
                for &child in current.children.iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
        self.stack = stack;
    }

    fn stop_scan(&mut self) {
        self.scan_cancel.store(true, AtomicOrdering::Relaxed);
        self.scan_rx = None;
        if let Some(handle) = self.scan_handle.take() {
            let _ = handle.join();
        }
        self.scanning = false;
    }

    fn start_scan(&mut self) {
        self.stop_scan();
        self.files.clear();
        self.truncated = false;
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        let excluder = Arc::clone(&self.excluder);
        let flag = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("index-projet".to_string())
            .spawn(move || walk_sources(root, excluder, flag, sender));
        match spawned {
            Ok(handle) => {
                self.scan_cancel = cancel;
                self.scan_handle = Some(handle);
                self.scan_rx = Some(receiver);
                self.scanning = true;
            }
            Err(_) => {
                self.scanning = false;
            }
        }
    }

    fn start_targets(&mut self) {
        self.targets.clear();
        self.targets_error = None;
        self.targets_rx = None;
        self.targets_pending = false;
        if self.kind == Kind::Plain {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        let kind = self.kind;
        let excluder = Arc::clone(&self.excluder);
        let spawned = std::thread::Builder::new()
            .name("cibles-projet".to_string())
            .spawn(move || {
                let _ = sender.send(collect_targets(root, kind, excluder));
            });
        match spawned {
            Ok(_) => {
                self.targets_rx = Some(receiver);
                self.targets_pending = true;
            }
            Err(error) => {
                self.targets_error = Some(format!("cibles indisponibles: {error}"));
            }
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        self.scan_cancel.store(true, AtomicOrdering::Relaxed);
        self.scan_rx = None;
        if let Some(handle) = self.scan_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const TEXT3D: &str = "/Users/anna/tmp/text3d";
    const GODOT: &str = "/Users/anna/tmp/godot_profiler";
    const ABYSSAL: &str = "/Users/anna/RustroverProjects/abyssal_engine";
    const NDJSON: &str = "/Users/anna/tmp/csharp_ndjson";

    fn settle(project: &mut Project) -> Duration {
        let start = Instant::now();
        while project.poll() {
            if start.elapsed() > Duration::from_secs(300) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        start.elapsed()
    }

    fn temporary(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("text3d-projet-{name}"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("dossier temporaire");
        normalize(directory)
    }

    #[test]
    fn joker_couvre_etoile_et_point_interrogation() {
        assert!(wildcard("*.ppm", "image.ppm"));
        assert!(!wildcard("*.ppm", "image.png"));
        assert!(wildcard("*", ""));
        assert!(wildcard("a*b*c", "abbbc"));
        assert!(!wildcard("a*b*c", "abbb"));
        assert!(wildcard("te?t", "test"));
        assert!(!wildcard("te?t", "teest"));
        assert!(wildcard("tests/refs/*.ppm", "tests/refs/a.ppm"));
        assert!(!wildcard("", "x"));
        assert!(wildcard("", ""));
    }

    #[test]
    fn tri_dossiers_puis_noms_insensibles_a_la_casse() {
        assert_eq!(compare_names("Alpha", "alpha"), Ordering::Less);
        assert_eq!(compare_names("alpha", "Beta"), Ordering::Less);
        assert_eq!(compare_names("Zeta", "beta"), Ordering::Greater);
        assert_eq!(compare_names("abc", "abcd"), Ordering::Less);
    }

    #[test]
    fn extension_comparee_sans_casse() {
        assert!(has_extension("Projet.CSPROJ", "csproj"));
        assert!(has_extension("a.sln", "sln"));
        assert!(!has_extension("sln", "sln"));
        assert!(!has_extension("a.slnx", "sln"));
        assert!(is_project_file("Deep.fsproj"));
        assert!(!is_project_file("Deep.txt"));
        assert!(is_source(Path::new("/x/y.WGSL")));
        assert!(!is_source(Path::new("/x/y.png")));
        assert!(!is_source(Path::new("/x/Makefile")));
    }

    #[test]
    fn gitignore_gere_les_formes_courantes() {
        let root = temporary("gitignore");
        fs::write(
            root.join(".gitignore"),
            "# commentaire\n\n/build\ncache/\n*.ppm\n!garde.ppm\n/tests/refs/*.tmp\nnote.txt\n",
        )
        .expect("ecriture");
        let excluder = Excluder::load(&root);
        let mut scratch = String::new();
        assert!(excluder.excluded(&root.join("build"), "build", true, &root, &mut scratch));
        assert!(!excluder.excluded(&root.join("src/build"), "build", true, &root, &mut scratch));
        assert!(excluder.excluded(&root.join("a/cache"), "cache", true, &root, &mut scratch));
        assert!(!excluder.excluded(&root.join("a/cache"), "cache", false, &root, &mut scratch));
        assert!(excluder.excluded(&root.join("a/b/x.ppm"), "x.ppm", false, &root, &mut scratch));
        assert!(excluder.excluded(
            &root.join("garde.ppm"),
            "garde.ppm",
            false,
            &root,
            &mut scratch
        ));
        assert!(excluder.excluded(
            &root.join("tests/refs/z.tmp"),
            "z.tmp",
            false,
            &root,
            &mut scratch
        ));
        assert!(!excluder.excluded(
            &root.join("tests/z.tmp"),
            "z.tmp",
            false,
            &root,
            &mut scratch
        ));
        assert!(excluder.excluded(
            &root.join("d/note.txt"),
            "note.txt",
            false,
            &root,
            &mut scratch
        ));
        assert!(excluder.excluded(&root.join("target"), "target", true, &root, &mut scratch));
        assert!(excluder.excluded(&root.join("obj"), "obj", true, &root, &mut scratch));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cadre_cible_lu_sans_dependance_xml() {
        let single = "<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>";
        assert_eq!(target_framework(single), Some("net8.0"));
        let multiple = "<TargetFrameworks>net8.0;netstandard2.1</TargetFrameworks>";
        assert_eq!(target_framework(multiple), Some("net8.0;netstandard2.1"));
        assert_eq!(target_framework("<Project></Project>"), None);
        assert_eq!(target_framework("<TargetFramework>net8.0"), None);
        assert_eq!(
            target_framework("<TargetFramework>   </TargetFramework>"),
            None
        );
    }

    #[test]
    fn arbre_paresseux_trie_et_repliable() {
        let root = temporary("arbre");
        fs::create_dir_all(root.join("zeta")).expect("dossier");
        fs::create_dir_all(root.join("Alpha/interne")).expect("dossier");
        fs::create_dir_all(root.join("target/debug")).expect("dossier");
        fs::write(root.join("b.rs"), "fn main() {}").expect("fichier");
        fs::write(root.join("A.rs"), "fn main() {}").expect("fichier");
        fs::write(root.join("Alpha/interne/c.rs"), "fn c() {}").expect("fichier");
        fs::write(root.join("target/debug/d.rs"), "fn d() {}").expect("fichier");

        let mut project = Project::open(&root);
        let names: Vec<&str> = project
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["text3d-projet-arbre", "Alpha", "zeta", "A.rs", "b.rs"]
        );
        assert_eq!(project.entries()[0].depth, 0);
        assert_eq!(project.entries()[1].depth, 1);
        assert!(!project.entries()[1].expanded);

        project.toggle(1);
        let names: Vec<&str> = project
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "text3d-projet-arbre",
                "Alpha",
                "interne",
                "zeta",
                "A.rs",
                "b.rs"
            ]
        );
        assert_eq!(project.entries()[2].depth, 2);

        project.toggle(1);
        assert_eq!(project.entries().len(), 5);

        project.expand_to(&root.join("Alpha/interne/c.rs"));
        let names: Vec<&str> = project
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "text3d-projet-arbre",
                "Alpha",
                "interne",
                "c.rs",
                "zeta",
                "A.rs",
                "b.rs"
            ]
        );

        project.toggle(0);
        assert_eq!(project.entries().len(), 1);
        project.toggle(0);
        assert_eq!(project.entries().len(), 7);

        settle(&mut project);
        let (count, done) = project.indexed();
        assert!(done);
        assert_eq!(count, 3);
        assert!(
            !project
                .files()
                .iter()
                .any(|path| path.starts_with(root.join("target")))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ouverture_sur_un_fichier_remonte_au_marqueur() {
        let root = temporary("marqueur");
        fs::create_dir_all(root.join("src/interne")).expect("dossier");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("fichier");
        fs::write(root.join("src/interne/lib.rs"), "pub fn x() {}").expect("fichier");
        let project = Project::open(&root.join("src/interne/lib.rs"));
        assert_eq!(project.root(), root.as_path());
        assert_eq!(project.kind(), Kind::Rust);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn racine_de_serveur_suit_l_espace_de_travail() {
        let root = temporary("espace");
        fs::create_dir_all(root.join("src")).expect("dossier");
        fs::create_dir_all(root.join("derive/src")).expect("dossier");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"racine\"\n\n[workspace]\nmembers = [\"derive\"]\n",
        )
        .expect("fichier");
        fs::write(
            root.join("derive/Cargo.toml"),
            "[package]\nname = \"derive\"\n",
        )
        .expect("fichier");
        fs::write(root.join("src/lib.rs"), "pub fn x() {}").expect("fichier");
        fs::write(root.join("derive/src/lib.rs"), "pub fn y() {}").expect("fichier");

        let project = Project::open(&root);
        let membre = project
            .server_for(&root.join("derive/src/lib.rs"))
            .expect("serveur pour le membre");
        assert_eq!(membre.0, ServerKind::Rust);
        assert_eq!(membre.1, root);
        let racine = project
            .server_for(&root.join("src/lib.rs"))
            .expect("serveur pour la racine");
        assert_eq!(racine.1, root);
        assert!(project.server_for(&root.join("README.md")).is_none());
        assert!(project.server_for(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn racine_csharp_prefere_la_solution() {
        let root = temporary("solution");
        fs::create_dir_all(root.join("src/App")).expect("dossier");
        fs::write(
            root.join("Tout.sln"),
            "Microsoft Visual Studio Solution File",
        )
        .expect("fichier");
        fs::write(
            root.join("src/App/App.csproj"),
            "<Project><PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup></Project>",
        )
        .expect("fichier");
        fs::write(root.join("src/App/Program.cs"), "class P {}").expect("fichier");
        let project = Project::open(&root);
        let found = project
            .server_for(&root.join("src/App/Program.cs"))
            .expect("serveur c#");
        assert_eq!(found.0, ServerKind::CSharp);
        assert_eq!(found.1, root);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn projets_de_la_solution_extraits() {
        let root = temporary("references");
        fs::create_dir_all(root.join("src/NdJson")).expect("dossier");
        let solution = root.join("NdJson.sln");
        fs::write(
            &solution,
            "Microsoft Visual Studio Solution File, Format Version 12.00\r\n\
Project(\"{2150E333-8FDC-42A3-9474-1A3956D46DE8}\") = \"src\", \"src\", \"{827E0CD3}\"\r\nEndProject\r\n\
Project(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"NdJson\", \"src\\NdJson\\NdJson.csproj\", \"{93FC3130}\"\r\nEndProject\r\n",
        )
        .expect("fichier");
        fs::write(
            root.join("src/NdJson/NdJson.csproj"),
            "<Project><PropertyGroup><TargetFramework>netstandard2.1</TargetFramework></PropertyGroup></Project>",
        )
        .expect("fichier");
        let mut found = Vec::new();
        solution_projects(&solution, &mut found);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], normalize(root.join("src/NdJson/NdJson.csproj")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn projet_sans_marqueur_reste_simple() {
        let root = temporary("simple");
        fs::write(root.join("notes.txt"), "bonjour").expect("fichier");
        let mut project = Project::open(&root);
        assert_eq!(project.kind(), Kind::Plain);
        settle(&mut project);
        assert!(project.targets().is_empty());
        assert!(project.targets_error().is_none());
        assert_eq!(project.indexed(), (1, true));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn projet_mixte_detecte_les_deux_langages() {
        let root = temporary("mixte");
        fs::create_dir_all(root.join("outil")).expect("dossier");
        fs::write(root.join("Jeu.csproj"), "<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>").expect("fichier");
        fs::write(
            root.join("outil/Cargo.toml"),
            "[package]\nname = \"outil\"\n",
        )
        .expect("fichier");
        let project = Project::open(&root);
        assert_eq!(project.kind(), Kind::Mixed);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rafraichir_conserve_les_dossiers_ouverts() {
        let root = temporary("rafraichir");
        fs::create_dir_all(root.join("un/deux")).expect("dossier");
        fs::write(root.join("un/deux/a.rs"), "fn a() {}").expect("fichier");
        let mut project = Project::open(&root);
        project.expand_to(&root.join("un/deux/a.rs"));
        let avant = project.entries().len();
        fs::write(root.join("un/deux/b.rs"), "fn b() {}").expect("fichier");
        project.refresh();
        assert_eq!(project.entries().len(), avant + 1);
        settle(&mut project);
        assert_eq!(project.indexed(), (2, true));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn projet_rust_reel_text3d() {
        let root = Path::new(TEXT3D);
        if !root.is_dir() {
            return;
        }
        let mut project = Project::open(root);
        assert_eq!(project.kind(), Kind::Rust);
        assert_eq!(project.root(), normalize(root.to_path_buf()).as_path());
        assert_eq!(project.label(), "text3d");
        let duration = settle(&mut project);
        let (count, done) = project.indexed();
        assert!(done);
        assert!(count > 10, "index trop petit: {count}");
        assert!(duration < Duration::from_secs(60));
        assert!(
            project
                .files()
                .iter()
                .any(|path| path.ends_with("src/project.rs"))
        );
        assert!(
            !project
                .files()
                .iter()
                .any(|path| path.components().any(|c| c.as_os_str() == "target")),
            "target/ n a pas ete exclu"
        );
        assert!(
            !project
                .entries()
                .iter()
                .any(|entry| entry.name == "target" || entry.name == ".git")
        );
        assert!(
            !project
                .files()
                .iter()
                .any(|path| path.ends_with("text3d.png")),
            "le gitignore n a pas ete applique"
        );
        assert!(
            project
                .targets()
                .iter()
                .any(|target| target.name == "text3d"
                    && target.kind == "bin"
                    && target.manifest.ends_with("Cargo.toml"))
        );
        let server = project
            .server_for(&root.join("src/main.rs"))
            .expect("serveur rust");
        assert_eq!(server.0, ServerKind::Rust);
        assert_eq!(server.1, normalize(root.to_path_buf()));
    }

    #[test]
    fn projet_csharp_reel_godot_profiler() {
        let root = Path::new(GODOT);
        if !root.is_dir() {
            return;
        }
        let mut project = Project::open(root);
        assert_eq!(project.kind(), Kind::CSharp);
        settle(&mut project);
        let (count, done) = project.indexed();
        assert!(done);
        assert!(count > 5, "index trop petit: {count}");
        assert!(
            !project
                .files()
                .iter()
                .any(|path| path.components().any(|c| {
                    let name = c.as_os_str();
                    name == "obj" || name == "bin" || name == ".godot"
                })),
            "obj/ bin/ ou .godot/ n ont pas ete exclus"
        );
        assert!(
            project
                .targets()
                .iter()
                .any(|target| target.name == "DeepProfiler" && target.kind == "net8.0"),
            "cibles: {:?}",
            project
                .targets()
                .iter()
                .map(|t| (t.name.as_str(), t.kind.as_str()))
                .collect::<Vec<_>>()
        );
        let source = project
            .files()
            .iter()
            .find(|path| path.extension().map(|e| e == "cs").unwrap_or(false))
            .cloned()
            .expect("au moins un fichier c#");
        let server = project.server_for(&source).expect("serveur c#");
        assert_eq!(server.0, ServerKind::CSharp);
        assert_eq!(server.1, normalize(root.to_path_buf()));
    }

    #[test]
    fn espace_de_travail_rust_reel_abyssal() {
        let root = Path::new(ABYSSAL);
        if !root.is_dir() {
            return;
        }
        let mut project = Project::open(root);
        assert_eq!(project.kind(), Kind::Rust);
        let duration = settle(&mut project);
        let (count, done) = project.indexed();
        assert!(done);
        assert!(count > 100, "index trop petit: {count}");
        assert!(duration < Duration::from_secs(300));
        assert!(
            !project
                .files()
                .iter()
                .any(|path| path.components().any(|c| c.as_os_str() == "target")),
            "target/ n a pas ete exclu"
        );
        assert!(
            project
                .targets()
                .iter()
                .any(|target| target.name == "abyssal_engine")
        );
        assert!(
            project
                .targets()
                .iter()
                .any(|target| target.name == "abyssal_derive"
                    && target.manifest.ends_with("derive/Cargo.toml")),
            "le membre derive est absent des cibles"
        );
        let membre = project
            .server_for(&root.join("derive/src/lib.rs"))
            .expect("serveur pour derive");
        assert_eq!(membre.1, normalize(root.to_path_buf()));
        let principal = project
            .server_for(&root.join("src/lib.rs"))
            .expect("serveur pour la racine");
        assert_eq!(principal.1, normalize(root.to_path_buf()));
    }

    #[test]
    fn solution_reelle_ndjson() {
        let root = Path::new(NDJSON);
        if !root.is_dir() {
            return;
        }
        let mut project = Project::open(root);
        assert_eq!(project.kind(), Kind::CSharp);
        settle(&mut project);
        assert!(
            project.targets().len() >= 4,
            "cibles: {}",
            project.targets().len()
        );
        assert!(
            project
                .targets()
                .iter()
                .any(|target| target.name == "NdJson")
        );
        let server = project
            .server_for(&root.join("src/NdJson/Attributes.cs"))
            .expect("serveur c#");
        assert_eq!(server.1, normalize(root.to_path_buf()));
    }
}
