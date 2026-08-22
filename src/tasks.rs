use crate::json::Json;
use crate::lsp::protocol::{Diagnostic, Position, Range, Severity};
use crate::project::{Kind, Project};
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

const MAX_LINES: usize = 20_000;
const LINE_DROP: usize = 4_096;
const MAX_DIAGNOSTICS: usize = 8_192;
const MAX_PER_POLL: usize = 4_096;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const READ_BUFFER: usize = 64 * 1024;
const SIGNAL_KILL: i32 = 9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    Check,
    Build,
    Test,
    Run,
    Clippy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Out,
    Err,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    pub stream: Stream,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Cargo,
    Dotnet,
    Plain,
}

enum Tool {
    Cargo,
    Dotnet(Option<PathBuf>),
}

enum Message {
    Text(Line),
    Found(PathBuf, Diagnostic),
}

pub struct TaskRunner {
    child: Option<Child>,
    receiver: Option<Receiver<Message>>,
    readers: Vec<JoinHandle<()>>,
    lines: Vec<Line>,
    diagnostics: Vec<(PathBuf, Diagnostic)>,
    label: String,
    exit: Option<i32>,
}

impl TaskRunner {
    pub fn new() -> TaskRunner {
        TaskRunner {
            child: None,
            receiver: None,
            readers: Vec::new(),
            lines: Vec::new(),
            diagnostics: Vec::new(),
            label: String::new(),
            exit: None,
        }
    }

    pub fn start(&mut self, task: Task, project: &Project) -> Result<(), String> {
        if self.running() {
            return Err("une tache est deja en cours".to_string());
        }
        let root = project.root();
        if !root.is_dir() {
            return Err(format!("racine de projet introuvable : {}", root.display()));
        }
        let tool = select_tool(project, root)?;
        let program = match tool {
            Tool::Cargo => locate("cargo", &["~/.cargo/bin/cargo", "/usr/local/bin/cargo"])?,
            Tool::Dotnet(_) => locate(
                "dotnet",
                &[
                    "~/.dotnet/dotnet",
                    "/usr/local/share/dotnet/dotnet",
                    "/usr/local/bin/dotnet",
                    "/opt/homebrew/bin/dotnet",
                ],
            )?,
        };
        let (arguments, label, format) = plan(task, &tool);
        self.launch(&program, &arguments, root, format, label)
    }

    pub fn poll(&mut self) -> bool {
        if let Some(receiver) = self.receiver.take() {
            let mut budget = MAX_PER_POLL;
            let mut connected = true;
            while budget > 0 {
                match receiver.try_recv() {
                    Ok(message) => {
                        self.absorb(message);
                        budget -= 1;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        connected = false;
                        break;
                    }
                }
            }
            if connected {
                self.receiver = Some(receiver);
            }
        }
        if self.receiver.is_none() {
            let outcome = match self.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(status.code()),
                    Ok(None) => None,
                    Err(_) => Some(None),
                },
                None => None,
            };
            if let Some(code) = outcome {
                self.child = None;
                self.exit = code;
                self.join_readers();
            }
        }
        self.running()
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn diagnostics(&self) -> &[(PathBuf, Diagnostic)] {
        &self.diagnostics
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn exit_code(&self) -> Option<i32> {
        if self.child.is_some() {
            None
        } else {
            self.exit
        }
    }

    pub fn kill(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        terminate(&mut child);
        self.exit = match child.wait() {
            Ok(status) => status.code(),
            Err(_) => None,
        };
        self.join_readers();
        if let Some(receiver) = self.receiver.take() {
            let mut budget = MAX_PER_POLL;
            while budget > 0 {
                match receiver.try_recv() {
                    Ok(message) => {
                        self.absorb(message);
                        budget -= 1;
                    }
                    Err(_) => break,
                }
            }
        }
        self.push_line("tache interrompue".to_string(), Stream::Err);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.diagnostics.clear();
        if !self.running() {
            self.exit = None;
        }
    }

    fn launch(
        &mut self,
        program: &Path,
        arguments: &[String],
        root: &Path,
        format: Format,
        label: String,
    ) -> Result<(), String> {
        if self.running() {
            return Err("une tache est deja en cours".to_string());
        }
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CARGO_TERM_COLOR", "never")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("MSBUILDDISABLENODEREUSE", "1");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("impossible de lancer {} : {}", program.display(), error))?;
        let output = child.stdout.take();
        let errors = child.stderr.take();
        let (output, errors) = match (output, errors) {
            (Some(output), Some(errors)) => (output, errors),
            _ => {
                terminate(&mut child);
                let _ = child.wait();
                return Err("impossible de brancher les sorties du processus".to_string());
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.lines.clear();
        self.diagnostics.clear();
        self.exit = None;
        self.label = label;
        self.readers.clear();
        self.readers.reserve(2);
        self.readers.push(spawn_reader(
            output,
            Stream::Out,
            format,
            root.to_path_buf(),
            sender.clone(),
        ));
        self.readers.push(spawn_reader(
            errors,
            Stream::Err,
            Format::Plain,
            root.to_path_buf(),
            sender,
        ));
        self.receiver = Some(receiver);
        self.child = Some(child);
        Ok(())
    }

    fn absorb(&mut self, message: Message) {
        match message {
            Message::Text(line) => {
                if self.lines.len() >= MAX_LINES {
                    self.lines.drain(..LINE_DROP);
                }
                self.lines.push(line);
            }
            Message::Found(path, diagnostic) => {
                if self.diagnostics.len() < MAX_DIAGNOSTICS {
                    self.diagnostics.push((path, diagnostic));
                }
            }
        }
    }

    fn push_line(&mut self, text: String, stream: Stream) {
        self.absorb(Message::Text(Line { text, stream }));
    }

    fn join_readers(&mut self) {
        for handle in self.readers.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Default for TaskRunner {
    fn default() -> TaskRunner {
        TaskRunner::new()
    }
}

impl Drop for TaskRunner {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.kill();
        }
    }
}

fn select_tool(project: &Project, root: &Path) -> Result<Tool, String> {
    let cargo = root.join("Cargo.toml").is_file();
    match project.kind() {
        Kind::Rust => Ok(Tool::Cargo),
        Kind::CSharp => Ok(Tool::Dotnet(find_project_file(root))),
        Kind::Mixed => {
            if cargo {
                Ok(Tool::Cargo)
            } else {
                match find_project_file(root) {
                    Some(target) => Ok(Tool::Dotnet(Some(target))),
                    None => Err("aucun manifeste cargo ou dotnet a cette racine".to_string()),
                }
            }
        }
        Kind::Plain => {
            if cargo {
                return Ok(Tool::Cargo);
            }
            match find_project_file(root) {
                Some(target) => Ok(Tool::Dotnet(Some(target))),
                None => Err("aucun manifeste cargo ou dotnet a cette racine".to_string()),
            }
        }
    }
}

fn find_project_file(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut solution: Option<PathBuf> = None;
    let mut project: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let slot =
            if extension.eq_ignore_ascii_case("sln") || extension.eq_ignore_ascii_case("slnx") {
                &mut solution
            } else if is_project_extension(extension) {
                &mut project
            } else {
                continue;
            };
        let replace = match slot.as_ref() {
            Some(current) => path < *current,
            None => true,
        };
        if replace {
            *slot = Some(path);
        }
    }
    solution.or(project)
}

fn is_project_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("csproj")
        || extension.eq_ignore_ascii_case("fsproj")
        || extension.eq_ignore_ascii_case("vbproj")
}

fn is_project_file(path: &Path) -> bool {
    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) => is_project_extension(extension),
        None => false,
    }
}

fn plan(task: Task, tool: &Tool) -> (Vec<String>, String, Format) {
    match tool {
        Tool::Cargo => {
            let (verb, all_targets) = match task {
                Task::Check => ("check", true),
                Task::Clippy => ("clippy", true),
                Task::Build => ("build", false),
                Task::Test => ("test", false),
                Task::Run => ("run", false),
            };
            let mut arguments = Vec::with_capacity(3);
            arguments.push(verb.to_string());
            arguments.push("--message-format=json".to_string());
            if all_targets {
                arguments.push("--all-targets".to_string());
            }
            (arguments, format!("cargo {verb}"), Format::Cargo)
        }
        Tool::Dotnet(target) => {
            let mut arguments = Vec::with_capacity(6);
            let label = match task {
                Task::Run => {
                    arguments.push("run".to_string());
                    if let Some(path) = target.as_ref().filter(|value| is_project_file(value)) {
                        arguments.push("--project".to_string());
                        arguments.push(path.to_string_lossy().into_owned());
                    }
                    "dotnet run".to_string()
                }
                Task::Test => {
                    arguments.push("test".to_string());
                    arguments.push("--nologo".to_string());
                    if let Some(path) = target {
                        arguments.push(path.to_string_lossy().into_owned());
                    }
                    "dotnet test".to_string()
                }
                Task::Clippy => {
                    arguments.push("build".to_string());
                    arguments.push("--nologo".to_string());
                    if let Some(path) = target {
                        arguments.push(path.to_string_lossy().into_owned());
                    }
                    arguments.push("-p:EnforceCodeStyleInBuild=true".to_string());
                    arguments.push("-p:RunAnalyzersDuringBuild=true".to_string());
                    "dotnet build analyse".to_string()
                }
                Task::Check | Task::Build => {
                    arguments.push("build".to_string());
                    arguments.push("--nologo".to_string());
                    if let Some(path) = target {
                        arguments.push(path.to_string_lossy().into_owned());
                    }
                    "dotnet build".to_string()
                }
            };
            (arguments, label, Format::Dotnet)
        }
    }
}

fn locate(name: &str, fallbacks: &[&str]) -> Result<PathBuf, String> {
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(name);
            if is_runnable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME");
    for entry in fallbacks {
        let candidate = match entry.strip_prefix("~/") {
            Some(rest) => match home.as_ref() {
                Some(base) => PathBuf::from(base).join(rest),
                None => continue,
            },
            None => PathBuf::from(entry),
        };
        if is_runnable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(format!("programme introuvable : {name}"))
}

fn is_runnable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "kill"]
    fn send_signal(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
    let pid = child.id() as i32;
    if pid > 1 {
        unsafe {
            send_signal(-pid, SIGNAL_KILL);
        }
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
}

fn spawn_reader<R>(
    source: R,
    stream: Stream,
    format: Format,
    root: PathBuf,
    sender: Sender<Message>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut reader = BufReader::with_capacity(READ_BUFFER, source);
        let mut raw: Vec<u8> = Vec::with_capacity(512);
        let mut state = Sink::new(root, sender, format);
        loop {
            raw.clear();
            match reader.read_until(b'\n', &mut raw) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            while matches!(raw.last(), Some(b'\n') | Some(b'\r')) {
                raw.pop();
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            if !state.handle(text, stream) {
                break;
            }
        }
    })
}

struct Sink {
    root: PathBuf,
    sender: Sender<Message>,
    format: Format,
    sources: HashMap<PathBuf, Option<FileLines>>,
    resolved: HashMap<String, PathBuf>,
    seen: HashSet<u64>,
    scratch: String,
}

impl Sink {
    fn new(root: PathBuf, sender: Sender<Message>, format: Format) -> Sink {
        Sink {
            root,
            sender,
            format,
            sources: HashMap::new(),
            resolved: HashMap::new(),
            seen: HashSet::new(),
            scratch: String::new(),
        }
    }

    fn handle(&mut self, text: String, stream: Stream) -> bool {
        match self.format {
            Format::Plain => self.emit_owned(text, stream),
            Format::Dotnet => {
                if let Some((path, diagnostic)) = parse_dotnet(&text, &self.root)
                    && !self.emit_diagnostic(path, diagnostic)
                {
                    return false;
                }
                self.emit_owned(text, stream)
            }
            Format::Cargo => self.handle_cargo(text, stream),
        }
    }

    fn handle_cargo(&mut self, text: String, stream: Stream) -> bool {
        let Ok(parsed) = Json::parse(&text) else {
            return self.emit_owned(text, stream);
        };
        let Some(reason) = parsed.get("reason").and_then(Json::as_str) else {
            return self.emit_owned(text, stream);
        };
        if reason != "compiler-message" {
            return true;
        }
        let Some(message) = parsed.get("message") else {
            return true;
        };
        let level = message
            .get("level")
            .and_then(Json::as_str)
            .unwrap_or("error");
        let summary = message
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let rendered = message
            .get("rendered")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let code = message
            .path("code.code")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let source = if code.starts_with("clippy::") {
            "clippy"
        } else {
            "rustc"
        };
        strip_ansi(rendered, &mut self.scratch);
        while self.scratch.ends_with('\n') || self.scratch.ends_with(' ') {
            self.scratch.pop();
        }
        let primary_message = if self.scratch.is_empty() {
            summary.to_string()
        } else {
            self.scratch.clone()
        };
        if self.scratch.is_empty() {
            if !summary.is_empty() && !self.emit_text(summary, stream) {
                return false;
            }
        } else {
            for piece in self.scratch.lines() {
                if !self.emit_text(piece, stream) {
                    return false;
                }
            }
        }
        let severity = severity_from_level(level);
        let Some(spans) = message.get("spans").and_then(Json::as_array) else {
            return true;
        };
        for span in spans {
            let Some(name) = span.get("file_name").and_then(Json::as_str) else {
                continue;
            };
            let primary = span
                .get("is_primary")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let path = self.resolve(name);
            let range = self.span_range(span, &path);
            let label = span.get("label").and_then(Json::as_str).unwrap_or_default();
            let (kind, body) = if primary {
                (severity, primary_message.clone())
            } else if label.is_empty() {
                (Severity::Hint, summary.to_string())
            } else {
                (Severity::Hint, label.to_string())
            };
            let diagnostic = Diagnostic {
                range,
                severity: kind,
                message: body,
                source: source.to_string(),
                code: code.to_string(),
            };
            if !self.emit_diagnostic(path, diagnostic) {
                return false;
            }
        }
        true
    }

    fn resolve(&mut self, name: &str) -> PathBuf {
        if let Some(found) = self.resolved.get(name) {
            return found.clone();
        }
        let candidate = Path::new(name);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let full = std::fs::canonicalize(&joined).unwrap_or(joined);
        self.resolved.insert(name.to_string(), full.clone());
        full
    }

    fn source(&mut self, path: &Path) -> Option<&FileLines> {
        if !self.sources.contains_key(path) {
            self.sources
                .insert(path.to_path_buf(), FileLines::load(path));
        }
        self.sources.get(path).and_then(|entry| entry.as_ref())
    }

    fn span_range(&mut self, span: &Json, path: &Path) -> Range {
        let line_start = span.get("line_start").and_then(Json::as_u32).unwrap_or(1);
        let line_end = span
            .get("line_end")
            .and_then(Json::as_u32)
            .unwrap_or(line_start);
        let column_start = span.get("column_start").and_then(Json::as_u32).unwrap_or(1);
        let column_end = span
            .get("column_end")
            .and_then(Json::as_u32)
            .unwrap_or(column_start);
        let byte_start = span
            .get("byte_start")
            .and_then(Json::as_i64)
            .and_then(|value| usize::try_from(value).ok());
        let byte_end = span
            .get("byte_end")
            .and_then(Json::as_i64)
            .and_then(|value| usize::try_from(value).ok());
        let raw_start = Position {
            line: line_start.saturating_sub(1),
            character: column_start.saturating_sub(1),
        };
        let raw_end = Position {
            line: line_end.saturating_sub(1),
            character: column_end.saturating_sub(1),
        };
        match self.source(path) {
            Some(file) => {
                let start = byte_start
                    .and_then(|offset| file.position_at_byte(offset))
                    .or_else(|| {
                        file.position_at_column(
                            raw_start.line as usize,
                            raw_start.character as usize,
                        )
                    })
                    .unwrap_or(raw_start);
                let end = byte_end
                    .and_then(|offset| file.position_at_byte(offset))
                    .or_else(|| {
                        file.position_at_column(raw_end.line as usize, raw_end.character as usize)
                    })
                    .unwrap_or(raw_end);
                Range { start, end }
            }
            None => Range {
                start: raw_start,
                end: raw_end,
            },
        }
    }

    fn emit_diagnostic(&mut self, path: PathBuf, diagnostic: Diagnostic) -> bool {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        diagnostic.range.start.line.hash(&mut hasher);
        diagnostic.range.start.character.hash(&mut hasher);
        diagnostic.code.hash(&mut hasher);
        diagnostic.message.hash(&mut hasher);
        if !self.seen.insert(hasher.finish()) {
            return true;
        }
        self.sender.send(Message::Found(path, diagnostic)).is_ok()
    }

    fn emit_text(&self, text: &str, stream: Stream) -> bool {
        self.sender
            .send(Message::Text(Line {
                text: text.to_string(),
                stream,
            }))
            .is_ok()
    }

    fn emit_owned(&self, text: String, stream: Stream) -> bool {
        self.sender
            .send(Message::Text(Line { text, stream }))
            .is_ok()
    }
}

fn severity_from_level(level: &str) -> Severity {
    match level {
        "error" | "error: internal compiler error" => Severity::Error,
        "warning" => Severity::Warning,
        "note" | "failure-note" => Severity::Information,
        _ => Severity::Hint,
    }
}

fn strip_ansi(input: &str, out: &mut String) {
    out.clear();
    if !input.as_bytes().contains(&0x1b) {
        out.push_str(input);
        return;
    }
    out.reserve(input.len());
    let bytes = input.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index += 1;
            if index < bytes.len() && bytes[index] == b'[' {
                index += 1;
                while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
                    index += 1;
                }
                if index < bytes.len() {
                    index += 1;
                }
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] != 0x1b {
            index += 1;
        }
        out.push_str(&input[start..index]);
    }
}

fn utf16_len(text: &str) -> u32 {
    let mut units = 0u32;
    for byte in text.as_bytes() {
        units += u32::from(*byte & 0xC0 != 0x80) + u32::from(*byte >= 0xF0);
    }
    units
}

struct FileLines {
    text: String,
    starts: Vec<u32>,
}

impl FileLines {
    fn load(path: &Path) -> Option<FileLines> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        Some(FileLines::from_text(text))
    }

    fn from_text(text: String) -> FileLines {
        let mut starts = Vec::with_capacity(text.len() / 24 + 1);
        starts.push(0u32);
        for (index, byte) in text.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                starts.push(index as u32 + 1);
            }
        }
        FileLines { text, starts }
    }

    fn line(&self, index: usize) -> Option<&str> {
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

    fn position_at_byte(&self, offset: usize) -> Option<Position> {
        if offset > self.text.len() {
            return None;
        }
        let index = match self.starts.binary_search(&(offset as u32)) {
            Ok(found) => found,
            Err(after) => after.checked_sub(1)?,
        };
        let start = *self.starts.get(index)? as usize;
        let prefix = self.text.get(start..offset)?;
        Some(Position {
            line: index as u32,
            character: utf16_len(prefix),
        })
    }

    fn position_at_column(&self, index: usize, column: usize) -> Option<Position> {
        let text = self.line(index)?;
        let mut units = 0u32;
        for character in text.chars().take(column) {
            units += character.len_utf16() as u32;
        }
        Some(Position {
            line: index as u32,
            character: units,
        })
    }
}

fn parse_dotnet(text: &str, root: &Path) -> Option<(PathBuf, Diagnostic)> {
    let trimmed = text.trim_start();
    for (index, _) in trimmed.match_indices("): ") {
        let head = &trimmed[..index];
        let Some(open) = head.rfind('(') else {
            continue;
        };
        let file = &head[..open];
        if file.is_empty() || Path::new(file).extension().is_none() {
            continue;
        }
        let mut values = [0u32; 4];
        let mut count = 0usize;
        let mut valid = open + 1 < head.len();
        for part in head[open + 1..].split(',') {
            if count == 4 {
                valid = false;
                break;
            }
            match part.parse::<u32>() {
                Ok(value) => {
                    values[count] = value;
                    count += 1;
                }
                Err(_) => {
                    valid = false;
                    break;
                }
            }
        }
        if !valid || (count != 2 && count != 4) {
            continue;
        }
        let rest = &trimmed[index + 3..];
        let (severity, after) = if let Some(tail) = rest.strip_prefix("error ") {
            (Severity::Error, tail)
        } else if let Some(tail) = rest.strip_prefix("warning ") {
            (Severity::Warning, tail)
        } else if let Some(tail) = rest.strip_prefix("info ") {
            (Severity::Information, tail)
        } else {
            continue;
        };
        let (code, body) = match after.find(':') {
            Some(at) => (after[..at].trim(), after[at + 1..].trim_start()),
            None => ("", after.trim_start()),
        };
        let mut message = body;
        if message.ends_with(']')
            && let Some(at) = message.rfind(" [")
        {
            let inner = &message[at + 2..message.len() - 1];
            if is_project_file(Path::new(inner.split("::").next().unwrap_or(inner)))
                || inner.contains(".sln")
            {
                message = message[..at].trim_end();
            }
        }
        let candidate = Path::new(file);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let path = std::fs::canonicalize(&joined).unwrap_or(joined);
        let start = Position {
            line: values[0].saturating_sub(1),
            character: values[1].saturating_sub(1),
        };
        let end = if count == 4 {
            Position {
                line: values[2].saturating_sub(1),
                character: values[3].saturating_sub(1),
            }
        } else {
            Position {
                line: start.line,
                character: start.character.saturating_add(1),
            }
        };
        let source = if code.starts_with("CS") {
            "csc"
        } else if code.starts_with("MSB") {
            "msbuild"
        } else {
            "dotnet"
        };
        return Some((
            path,
            Diagnostic {
                range: Range { start, end },
                severity,
                message: message.to_string(),
                source: source.to_string(),
                code: code.to_string(),
            },
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn workspace(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let directory = std::env::temp_dir().join(format!("text3d-tasks-{name}-{stamp}"));
        std::fs::create_dir_all(&directory).expect("dossier temporaire");
        directory
    }

    fn drain(receiver: &Receiver<Message>) -> (Vec<String>, Vec<(PathBuf, Diagnostic)>) {
        let mut texts = Vec::new();
        let mut found = Vec::new();
        while let Ok(message) = receiver.try_recv() {
            match message {
                Message::Text(line) => texts.push(line.text),
                Message::Found(path, diagnostic) => found.push((path, diagnostic)),
            }
        }
        (texts, found)
    }

    #[test]
    fn utf16_length_counts_units() {
        assert_eq!(utf16_len(""), 0);
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("élève"), 5);
        assert_eq!(utf16_len("日本語"), 3);
        assert_eq!(utf16_len("\u{1F600}"), 2);
        assert_eq!(utf16_len("a\u{1F600}b"), 4);
    }

    #[test]
    fn file_lines_maps_bytes_to_utf16() {
        let text = String::from("fn main() {\n    let r = \"élève \u{1F600}\".methode();\n}\n");
        let file = FileLines::from_text(text);
        assert_eq!(file.line(0), Some("fn main() {"));
        assert_eq!(file.line(2), Some("}"));
        assert_eq!(file.line(3), Some(""));
        assert_eq!(file.line(4), None);
        let start = file.position_at_byte(39).expect("position");
        assert_eq!(start.line, 1);
        assert_eq!(start.character, 23);
        let same = file.position_at_column(1, 22).expect("position");
        assert_eq!(same, start);
        assert_eq!(
            file.position_at_byte(0),
            Some(Position {
                line: 0,
                character: 0
            })
        );
        assert_eq!(file.position_at_byte(9_000), None);
    }

    #[test]
    fn file_lines_handles_carriage_returns() {
        let file = FileLines::from_text(String::from("une\r\ndeux\r\n"));
        assert_eq!(file.line(0), Some("une"));
        assert_eq!(file.line(1), Some("deux"));
        let position = file.position_at_byte(5).expect("position");
        assert_eq!(
            position,
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn strip_ansi_removes_sequences() {
        let mut out = String::new();
        strip_ansi("simple", &mut out);
        assert_eq!(out, "simple");
        strip_ansi("\u{1b}[0m\u{1b}[1;31merreur\u{1b}[0m ici", &mut out);
        assert_eq!(out, "erreur ici");
        strip_ansi("accents é conserves", &mut out);
        assert_eq!(out, "accents é conserves");
    }

    #[test]
    fn severity_mapping_is_complete() {
        assert_eq!(severity_from_level("error"), Severity::Error);
        assert_eq!(severity_from_level("warning"), Severity::Warning);
        assert_eq!(severity_from_level("note"), Severity::Information);
        assert_eq!(severity_from_level("help"), Severity::Hint);
        assert_eq!(severity_from_level("inconnu"), Severity::Hint);
    }

    #[test]
    fn dotnet_line_with_spaces_and_brackets() {
        let root = Path::new("/tmp");
        let raw = "/private/tmp/cs broken/demo/Casse Accent.cs(10,53): error CS1002: ; expected [/private/tmp/cs broken/DeepProfiler.csproj]";
        let (path, diagnostic) = parse_dotnet(raw, root).expect("diagnostic");
        assert!(path.to_string_lossy().ends_with("Casse Accent.cs"));
        assert_eq!(
            diagnostic.range.start,
            Position {
                line: 9,
                character: 52
            }
        );
        assert_eq!(
            diagnostic.range.end,
            Position {
                line: 9,
                character: 53
            }
        );
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.code, "CS1002");
        assert_eq!(diagnostic.source, "csc");
        assert_eq!(diagnostic.message, "; expected");
    }

    #[test]
    fn dotnet_line_variants() {
        let root = Path::new("/tmp/projet");
        let (path, warning) = parse_dotnet(
            "  demo/Vue.cs(3,7,3,19): warning CS0168: variable inutilisee",
            root,
        )
        .expect("avertissement");
        assert_eq!(path, PathBuf::from("/tmp/projet/demo/Vue.cs"));
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(
            warning.range.start,
            Position {
                line: 2,
                character: 6
            }
        );
        assert_eq!(
            warning.range.end,
            Position {
                line: 2,
                character: 18
            }
        );
        assert_eq!(warning.message, "variable inutilisee");

        let (_, sans_code) =
            parse_dotnet("/a/B.cs(1,1): error : quelque chose", root).expect("sans code");
        assert_eq!(sans_code.code, "");
        assert_eq!(sans_code.message, "quelque chose");
        assert_eq!(sans_code.source, "dotnet");

        let (_, msbuild) = parse_dotnet(
            "/a/B.csproj(4,2): error MSB4025: fichier illisible [/a/B.csproj]",
            root,
        )
        .expect("msbuild");
        assert_eq!(msbuild.source, "msbuild");
        assert_eq!(msbuild.message, "fichier illisible");

        let (_, garde) = parse_dotnet("/a/B.cs(1,1): error CS0001: attendu [ou pas]", root)
            .expect("crochets gardes");
        assert_eq!(garde.message, "attendu [ou pas]");

        let (_, cadre) = parse_dotnet(
            "/a/B.cs(1,1): error CS0002: souci [/a/B.csproj::TargetFramework=net8.0]",
            root,
        )
        .expect("cadre cible");
        assert_eq!(cadre.message, "souci");
    }

    #[test]
    fn dotnet_line_rejects_noise() {
        let root = Path::new("/tmp");
        assert!(parse_dotnet("Build succeeded.", root).is_none());
        assert!(parse_dotnet("    0 Warning(s)", root).is_none());
        assert!(parse_dotnet("MSBUILD : error MSB1011: trop de projets", root).is_none());
        assert!(parse_dotnet("  DeepProfiler -> /a/b/DeepProfiler.dll", root).is_none());
        assert!(parse_dotnet("truc(a,b): error CS1: non", root).is_none());
        assert!(parse_dotnet("dossier(1,2): error CS1: non", root).is_none());
        assert!(parse_dotnet("/a/B.cs(1,2): note CS1: non", root).is_none());
    }

    fn cargo_message(path: &str, spans: &str) -> String {
        format!(
            "{{\"reason\":\"compiler-message\",\"package_id\":\"p\",\"message\":{{\
\"message\":\"aucune methode nommee methode_absente_ici\",\
\"code\":{{\"code\":\"E0599\",\"explanation\":\"long\"}},\
\"level\":\"error\",\
\"rendered\":\"error[E0599]: aucune methode\\n  --> {path}:2:23\\n\",\
\"spans\":[{spans}]}}}}"
        )
    }

    #[test]
    fn cargo_span_uses_utf16_positions() {
        let directory = workspace("cargo");
        let file = directory.join("accents.rs");
        std::fs::write(
            &file,
            "fn main() {\n    let r = \"élève \u{1F600}\".methode_absente_ici();\n}\n",
        )
        .expect("ecriture");
        let name = file.to_string_lossy().into_owned();
        let spans = format!(
            "{{\"file_name\":\"{name}\",\"is_primary\":true,\"line_start\":2,\"line_end\":2,\
\"column_start\":23,\"column_end\":42,\"byte_start\":39,\"byte_end\":58,\"label\":null}}"
        );
        let (sender, receiver) = mpsc::channel();
        let mut sink = Sink::new(directory.clone(), sender, Format::Cargo);
        assert!(sink.handle(cargo_message(&name, &spans), Stream::Out));
        assert!(sink.handle(cargo_message(&name, &spans), Stream::Out));
        drop(sink);
        let (texts, found) = drain(&receiver);
        assert_eq!(found.len(), 1);
        assert_eq!(texts.len(), 4);
        assert_eq!(texts[0], "error[E0599]: aucune methode");
        let (reported, diagnostic) = &found[0];
        assert!(reported.ends_with("accents.rs"));
        assert_eq!(
            diagnostic.range.start,
            Position {
                line: 1,
                character: 23
            }
        );
        assert_eq!(
            diagnostic.range.end,
            Position {
                line: 1,
                character: 42
            }
        );
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.code, "E0599");
        assert_eq!(diagnostic.source, "rustc");
        assert!(diagnostic.message.starts_with("error[E0599]"));
        assert!(!diagnostic.message.ends_with('\n'));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn cargo_span_falls_back_to_columns() {
        let directory = workspace("colonnes");
        let file = directory.join("accents.rs");
        std::fs::write(
            &file,
            "fn main() {\n    let r = \"élève \u{1F600}\".methode_absente_ici();\n}\n",
        )
        .expect("ecriture");
        let name = file.to_string_lossy().into_owned();
        let spans = format!(
            "{{\"file_name\":\"{name}\",\"is_primary\":true,\"line_start\":2,\"line_end\":2,\
\"column_start\":23,\"column_end\":42}}"
        );
        let (sender, receiver) = mpsc::channel();
        let mut sink = Sink::new(directory.clone(), sender, Format::Cargo);
        assert!(sink.handle(cargo_message(&name, &spans), Stream::Out));
        drop(sink);
        let (_, found) = drain(&receiver);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].1.range.start,
            Position {
                line: 1,
                character: 23
            }
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn cargo_secondary_span_becomes_hint() {
        let directory = workspace("secondaire");
        let file = directory.join("simple.rs");
        std::fs::write(&file, "fn a() {}\nfn b() {}\n").expect("ecriture");
        let name = file.to_string_lossy().into_owned();
        let spans = format!(
            "{{\"file_name\":\"{name}\",\"is_primary\":true,\"line_start\":1,\"line_end\":1,\
\"column_start\":1,\"column_end\":3,\"byte_start\":0,\"byte_end\":2,\"label\":null}},\
{{\"file_name\":\"{name}\",\"is_primary\":false,\"line_start\":2,\"line_end\":2,\
\"column_start\":1,\"column_end\":3,\"byte_start\":10,\"byte_end\":12,\"label\":\"defini ici\"}}"
        );
        let (sender, receiver) = mpsc::channel();
        let mut sink = Sink::new(directory.clone(), sender, Format::Cargo);
        assert!(sink.handle(cargo_message(&name, &spans), Stream::Out));
        drop(sink);
        let (_, found) = drain(&receiver);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].1.severity, Severity::Hint);
        assert_eq!(found[1].1.message, "defini ici");
        assert_eq!(
            found[1].1.range.start,
            Position {
                line: 1,
                character: 0
            }
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn cargo_ignores_other_reasons_and_keeps_plain_lines() {
        let directory = workspace("bruit");
        let (sender, receiver) = mpsc::channel();
        let mut sink = Sink::new(directory.clone(), sender, Format::Cargo);
        assert!(sink.handle(
            "{\"reason\":\"build-finished\",\"success\":true}".to_string(),
            Stream::Out
        ));
        assert!(sink.handle("bonjour depuis le programme".to_string(), Stream::Out));
        assert!(sink.handle("{ ceci n est pas du json".to_string(), Stream::Out));
        drop(sink);
        let (texts, found) = drain(&receiver);
        assert!(found.is_empty());
        assert_eq!(
            texts,
            vec!["bonjour depuis le programme", "{ ceci n est pas du json"]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn plan_builds_expected_commands() {
        let (arguments, label, format) = plan(Task::Check, &Tool::Cargo);
        assert_eq!(
            arguments,
            vec!["check", "--message-format=json", "--all-targets"]
        );
        assert_eq!(label, "cargo check");
        assert!(format == Format::Cargo);
        let (arguments, label, _) = plan(Task::Clippy, &Tool::Cargo);
        assert_eq!(arguments[0], "clippy");
        assert_eq!(label, "cargo clippy");
        let (arguments, _, _) = plan(Task::Run, &Tool::Cargo);
        assert_eq!(arguments, vec!["run", "--message-format=json"]);
        let target = Some(PathBuf::from("/a/Projet.csproj"));
        let (arguments, label, format) = plan(Task::Build, &Tool::Dotnet(target.clone()));
        assert_eq!(arguments, vec!["build", "--nologo", "/a/Projet.csproj"]);
        assert_eq!(label, "dotnet build");
        assert!(format == Format::Dotnet);
        let (arguments, _, _) = plan(Task::Run, &Tool::Dotnet(target.clone()));
        assert_eq!(arguments, vec!["run", "--project", "/a/Projet.csproj"]);
        let (arguments, _, _) = plan(Task::Run, &Tool::Dotnet(Some(PathBuf::from("/a/T.sln"))));
        assert_eq!(arguments, vec!["run"]);
        let (arguments, label, _) = plan(Task::Clippy, &Tool::Dotnet(target));
        assert!(arguments.contains(&"-p:EnforceCodeStyleInBuild=true".to_string()));
        assert_eq!(label, "dotnet build analyse");
    }

    #[test]
    fn line_cap_drops_oldest() {
        let mut runner = TaskRunner::new();
        let total = MAX_LINES + 5_000;
        for index in 0..total {
            runner.push_line(format!("ligne {index}"), Stream::Out);
        }
        assert!(runner.lines().len() <= MAX_LINES);
        assert!(runner.lines().len() > MAX_LINES - LINE_DROP);
        assert_eq!(
            runner.lines().last().map(|line| line.text.as_str()),
            Some(format!("ligne {}", total - 1).as_str())
        );
        runner.clear();
        assert!(runner.lines().is_empty());
    }

    #[test]
    fn locate_reports_missing_program() {
        let outcome = locate("programme-qui-n-existe-vraiment-pas", &[]);
        assert!(outcome.is_err());
        let message = outcome.unwrap_err();
        assert!(message.starts_with("programme introuvable"));
    }

    #[cfg(unix)]
    fn wait_for(runner: &mut TaskRunner, limit: Duration) {
        let start = Instant::now();
        while runner.poll() {
            if start.elapsed() > limit {
                runner.kill();
                panic!("tache trop longue");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[cfg(unix)]
    #[test]
    fn runner_collects_both_streams_and_exit_code() {
        let directory = workspace("flux");
        let mut runner = TaskRunner::new();
        let arguments = vec![
            "-c".to_string(),
            "printf 'alpha\\nbeta\\n'; printf 'gamma\\n' 1>&2; exit 3".to_string(),
        ];
        runner
            .launch(
                Path::new("/bin/sh"),
                &arguments,
                &directory,
                Format::Plain,
                "essai".to_string(),
            )
            .expect("lancement");
        assert!(runner.running());
        assert_eq!(runner.label(), "essai");
        assert_eq!(runner.exit_code(), None);
        wait_for(&mut runner, Duration::from_secs(30));
        assert!(!runner.running());
        assert_eq!(runner.exit_code(), Some(3));
        let out: Vec<&str> = runner
            .lines()
            .iter()
            .filter(|line| line.stream == Stream::Out)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(out, vec!["alpha", "beta"]);
        let err: Vec<&str> = runner
            .lines()
            .iter()
            .filter(|line| line.stream == Stream::Err)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(err, vec!["gamma"]);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn runner_refuses_second_task() {
        let directory = workspace("occupe");
        let mut runner = TaskRunner::new();
        let arguments = vec!["-c".to_string(), "sleep 5".to_string()];
        runner
            .launch(
                Path::new("/bin/sh"),
                &arguments,
                &directory,
                Format::Plain,
                "premiere".to_string(),
            )
            .expect("lancement");
        let refus = runner.launch(
            Path::new("/bin/sh"),
            &arguments,
            &directory,
            Format::Plain,
            "seconde".to_string(),
        );
        assert_eq!(refus, Err("une tache est deja en cours".to_string()));
        runner.kill();
        assert!(!runner.running());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn kill_terminates_the_process_tree() {
        let directory = workspace("arbre");
        let mut runner = TaskRunner::new();
        let arguments = vec![
            "-c".to_string(),
            "sleep 120 & sleep 120 & printf 'demarre\\n'; wait".to_string(),
        ];
        runner
            .launch(
                Path::new("/bin/sh"),
                &arguments,
                &directory,
                Format::Plain,
                "arbre".to_string(),
            )
            .expect("lancement");
        let start = Instant::now();
        while runner.lines().is_empty() && start.elapsed() < Duration::from_secs(10) {
            runner.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runner.running());
        let killed = Instant::now();
        runner.kill();
        assert!(!runner.running());
        assert!(killed.elapsed() < Duration::from_secs(5));
        assert!(
            runner
                .lines()
                .last()
                .is_some_and(|line| line.text == "tache interrompue")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn heavy_output_stays_capped_and_responsive() {
        let directory = workspace("flot");
        let mut runner = TaskRunner::new();
        let arguments = vec!["-c".to_string(), "seq 1 200000".to_string()];
        runner
            .launch(
                Path::new("/bin/sh"),
                &arguments,
                &directory,
                Format::Plain,
                "flot".to_string(),
            )
            .expect("lancement");
        let start = Instant::now();
        let mut worst = Duration::ZERO;
        loop {
            let tick = Instant::now();
            let alive = runner.poll();
            let spent = tick.elapsed();
            if spent > worst {
                worst = spent;
            }
            if !alive {
                break;
            }
            if start.elapsed() > Duration::from_secs(120) {
                runner.kill();
                panic!("flot trop lent");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(runner.exit_code(), Some(0));
        assert!(runner.lines().len() <= MAX_LINES);
        assert!(runner.lines().len() > MAX_LINES - LINE_DROP);
        assert_eq!(
            runner.lines().last().map(|line| line.text.as_str()),
            Some("200000")
        );
        assert!(
            worst < Duration::from_millis(16),
            "poll le plus long {worst:?}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn runner_reports_missing_working_directory() {
        let mut runner = TaskRunner::new();
        let arguments = vec!["-c".to_string(), "true".to_string()];
        let outcome = runner.launch(
            Path::new("/bin/sh"),
            &arguments,
            Path::new("/dossier/qui/n/existe/pas"),
            Format::Plain,
            "absent".to_string(),
        );
        assert!(outcome.is_err());
        assert!(outcome.unwrap_err().starts_with("impossible de lancer"));
        assert!(!runner.running());
    }
}
