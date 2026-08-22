use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::json::Json;
use crate::lsp::protocol::{
    CompletionItem, Diagnostic, Location, Position, SignatureInfo, Symbol, TextEdit,
    parse_completion, parse_diagnostics, parse_document_symbols, parse_hover, parse_locations,
    parse_signature, parse_text_edits, parse_workspace_edit, parse_workspace_symbols, path_from_uri,
    uri_from_path,
};
use crate::lsp::transport::Transport;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CHANGE_COALESCE: Duration = Duration::from_millis(30);
const MESSAGES_PER_POLL: usize = 256;
const MAX_LOG_LINES: usize = 200;
const MAX_PROGRESS: usize = 16;
const MAX_STATUS: usize = 160;
const EXIT_LOG_LINES: usize = 6;

const CLIENT_CAPABILITIES: &str = r#"{"general":{"positionEncodings":["utf-16"]},"window":{"workDoneProgress":true},"workspace":{"applyEdit":false,"configuration":true,"workspaceFolders":true,"didChangeConfiguration":{"dynamicRegistration":false},"didChangeWatchedFiles":{"dynamicRegistration":false},"symbol":{"dynamicRegistration":false,"symbolKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]}}},"textDocument":{"synchronization":{"dynamicRegistration":false,"willSave":false,"willSaveWaitUntil":false,"didSave":true},"completion":{"dynamicRegistration":false,"contextSupport":true,"completionItem":{"snippetSupport":false,"commitCharactersSupport":false,"documentationFormat":["markdown","plaintext"],"deprecatedSupport":true,"preselectSupport":false,"insertReplaceSupport":true,"labelDetailsSupport":true},"completionItemKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]}},"hover":{"dynamicRegistration":false,"contentFormat":["markdown","plaintext"]},"signatureHelp":{"dynamicRegistration":false,"contextSupport":true,"signatureInformation":{"documentationFormat":["markdown","plaintext"],"activeParameterSupport":true,"parameterInformation":{"labelOffsetSupport":true}}},"definition":{"dynamicRegistration":false,"linkSupport":true},"references":{"dynamicRegistration":false},"documentSymbol":{"dynamicRegistration":false,"hierarchicalDocumentSymbolSupport":true,"symbolKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]}},"formatting":{"dynamicRegistration":false},"rename":{"dynamicRegistration":false,"prepareSupport":false},"publishDiagnostics":{"relatedInformation":true,"versionSupport":false,"codeDescriptionSupport":false,"dataSupport":false,"tagSupport":{"valueSet":[1,2]}}},"experimental":{"serverStatusNotification":true}}"#;

static NULL: Json = Json::Null;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServerKind {
    Rust,
    CSharp,
}

impl ServerKind {
    pub fn label(self) -> &'static str {
        match self {
            ServerKind::Rust => "rust-analyzer",
            ServerKind::CSharp => "csharp-ls",
        }
    }

    fn fallback_language(self) -> &'static str {
        match self {
            ServerKind::Rust => "rust",
            ServerKind::CSharp => "csharp",
        }
    }
}

pub enum Event {
    Ready,
    Status(String),
    Diagnostics { path: PathBuf, items: Vec<Diagnostic> },
    Completion { id: u64, items: Vec<CompletionItem>, incomplete: bool },
    Hover { id: u64, text: String },
    Definition { id: u64, locations: Vec<Location> },
    References { id: u64, locations: Vec<Location> },
    DocumentSymbols { id: u64, items: Vec<Symbol> },
    WorkspaceSymbols { id: u64, items: Vec<Symbol> },
    Signature { id: u64, info: Option<SignatureInfo> },
    Rename { id: u64, edits: Vec<(PathBuf, Vec<TextEdit>)> },
    Formatting { id: u64, edits: Vec<TextEdit> },
    Failed { id: u64, message: String },
    Exited { message: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Initialize,
    Shutdown,
    Completion,
    Hover,
    Definition,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    Signature,
    Rename,
    Formatting,
}

struct Pending {
    id: u64,
    kind: Kind,
    path: PathBuf,
    sent: Instant,
}

struct Doc {
    path: PathBuf,
    uri: String,
    version: i32,
    pending: Option<String>,
    spare: Option<String>,
    since: Instant,
}

#[derive(Default)]
struct Caps {
    completion: bool,
    hover: bool,
    definition: bool,
    references: bool,
    document_symbol: bool,
    workspace_symbol: bool,
    signature: bool,
    rename: bool,
    formatting: bool,
    save_text: bool,
}

pub struct Server {
    kind: ServerKind,
    root: PathBuf,
    transport: Transport,
    caps: Caps,
    ready: bool,
    exited: bool,
    status: String,
    line: String,
    token: String,
    payload: String,
    hover: String,
    next_id: u64,
    pending: Vec<Pending>,
    queued: Vec<String>,
    docs: Vec<Doc>,
    log: Vec<String>,
    drain: Vec<String>,
    progress: Vec<(String, String)>,
    rejected: Vec<(u64, String)>,
    failure: Option<String>,
}

impl Server {
    pub fn locate(kind: ServerKind) -> Option<PathBuf> {
        let name = kind.label();
        if let Some(found) = in_path(name) {
            return Some(found);
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        match kind {
            ServerKind::Rust => {
                if let Some(home) = home.as_ref() {
                    let candidate = home.join(".cargo").join("bin").join(name);
                    if runnable(&candidate) {
                        return Some(candidate);
                    }
                }
                let output = Command::new("rustup").args(["which", "rust-analyzer"]).output().ok()?;
                if !output.status.success() {
                    return None;
                }
                let text = String::from_utf8_lossy(&output.stdout);
                let candidate = PathBuf::from(text.trim());
                if runnable(&candidate) { Some(candidate) } else { None }
            }
            ServerKind::CSharp => {
                let home = home?;
                let candidate = home.join(".dotnet").join("tools").join(name);
                if runnable(&candidate) { Some(candidate) } else { None }
            }
        }
    }

    pub fn start(kind: ServerKind, root: &Path) -> Result<Server, String> {
        let program = Server::locate(kind)
            .ok_or_else(|| format!("serveur de langage introuvable: {}", kind.label()))?;
        let root = std::fs::canonicalize(root)
            .map_err(|error| format!("racine de projet illisible: {error}"))?;
        if !root.is_dir() {
            return Err("la racine de projet n est pas un dossier".to_string());
        }
        let transport = Transport::spawn(&program, &[], &root)?;
        let mut server = Server {
            kind,
            root,
            transport,
            caps: Caps::default(),
            ready: false,
            exited: false,
            status: "demarrage du serveur".to_string(),
            line: String::new(),
            token: String::new(),
            payload: String::with_capacity(4096),
            hover: String::new(),
            next_id: 1,
            pending: Vec::new(),
            queued: Vec::new(),
            docs: Vec::new(),
            log: Vec::new(),
            drain: Vec::new(),
            progress: Vec::new(),
            rejected: Vec::new(),
            failure: None,
        };
        server.send_initialize();
        Ok(server)
    }

    pub fn kind(&self) -> ServerKind {
        self.kind
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn poll(&mut self, out: &mut Vec<Event>) {
        if self.exited {
            return;
        }
        self.collect_logs();
        for entry in self.rejected.drain(..) {
            out.push(Event::Failed { id: entry.0, message: entry.1 });
        }
        if self.ready {
            let now = Instant::now();
            let mut index = 0;
            while index < self.docs.len() {
                let due = self.docs[index].pending.is_some()
                    && now.duration_since(self.docs[index].since) >= CHANGE_COALESCE;
                if due {
                    self.flush_doc(index);
                }
                index += 1;
            }
        }
        let mut handled = 0;
        while handled < MESSAGES_PER_POLL {
            let Some(message) = self.transport.poll() else {
                break;
            };
            self.handle(&message, out);
            handled += 1;
            if self.exited {
                return;
            }
        }
        self.check_timeouts(out);
        if let Some(error) = self.failure.take() {
            self.finish(error, out);
            return;
        }
        if !self.transport.alive() {
            self.finish("le serveur de langage s est arrete".to_string(), out);
        }
    }

    pub fn open(&mut self, path: &Path, text: &str) {
        let resolved = canonical(path);
        if let Some(index) = self.doc_index(&resolved) {
            self.store_change(index, text);
            return;
        }
        let uri = uri_from_path(&resolved);
        let language = language_id(&resolved, self.kind);
        self.docs.push(Doc {
            path: resolved,
            uri,
            version: 1,
            pending: None,
            spare: None,
            since: Instant::now(),
        });
        let index = self.docs.len() - 1;
        open_notification(&mut self.payload, "textDocument/didOpen");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str(",\"languageId\":");
        push_text(&mut self.payload, language);
        self.payload.push_str(",\"version\":1,\"text\":");
        push_text(&mut self.payload, text);
        self.payload.push_str("}}}");
        self.dispatch();
    }

    pub fn change(&mut self, path: &Path, text: &str) {
        let Some(index) = self.doc_index(path) else {
            return;
        };
        self.store_change(index, text);
    }

    pub fn save(&mut self, path: &Path, text: &str) {
        let Some(index) = self.doc_index(path) else {
            return;
        };
        self.store_change(index, text);
        self.flush_doc(index);
        open_notification(&mut self.payload, "textDocument/didSave");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push('}');
        if self.caps.save_text {
            self.payload.push_str(",\"text\":");
            push_text(&mut self.payload, text);
        }
        self.payload.push_str("}}");
        self.dispatch();
    }

    pub fn close(&mut self, path: &Path) {
        let Some(index) = self.doc_index(path) else {
            return;
        };
        open_notification(&mut self.payload, "textDocument/didClose");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("}}}");
        self.dispatch();
        self.docs.swap_remove(index);
    }

    pub fn completion(&mut self, path: &Path, position: Position, trigger: Option<char>) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.completion, "completion") else {
            return id;
        };
        open_request(&mut self.payload, id, "textDocument/completion");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("},\"position\":");
        push_position(&mut self.payload, position);
        match trigger {
            Some(character) => {
                let mut buffer = [0u8; 4];
                self.payload.push_str(",\"context\":{\"triggerKind\":2,\"triggerCharacter\":");
                push_text(&mut self.payload, character.encode_utf8(&mut buffer));
                self.payload.push('}');
            }
            None => self.payload.push_str(",\"context\":{\"triggerKind\":1}"),
        }
        self.payload.push_str("}}");
        self.dispatch();
        self.track(id, Kind::Completion, PathBuf::new());
        id
    }

    pub fn hover(&mut self, path: &Path, position: Position) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.hover, "survol") else {
            return id;
        };
        self.position_request(id, index, position, "textDocument/hover", Kind::Hover);
        id
    }

    pub fn definition(&mut self, path: &Path, position: Position) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.definition, "aller a la definition") else {
            return id;
        };
        self.position_request(id, index, position, "textDocument/definition", Kind::Definition);
        id
    }

    pub fn references(&mut self, path: &Path, position: Position) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.references, "references") else {
            return id;
        };
        open_request(&mut self.payload, id, "textDocument/references");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("},\"position\":");
        push_position(&mut self.payload, position);
        self.payload.push_str(",\"context\":{\"includeDeclaration\":true}}}");
        self.dispatch();
        self.track(id, Kind::References, PathBuf::new());
        id
    }

    pub fn document_symbols(&mut self, path: &Path) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.document_symbol, "symboles du document")
        else {
            return id;
        };
        open_request(&mut self.payload, id, "textDocument/documentSymbol");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("}}}");
        self.dispatch();
        let target = self.docs[index].path.clone();
        self.track(id, Kind::DocumentSymbols, target);
        id
    }

    pub fn workspace_symbols(&mut self, query: &str) -> u64 {
        let id = self.take_id();
        if self.ready && !self.caps.workspace_symbol {
            return self.reject(id, "symboles d espace de travail non pris en charge");
        }
        open_request(&mut self.payload, id, "workspace/symbol");
        self.payload.push_str("{\"query\":");
        push_text(&mut self.payload, query);
        self.payload.push_str("}}");
        self.dispatch();
        self.track(id, Kind::WorkspaceSymbols, PathBuf::new());
        id
    }

    pub fn signature(&mut self, path: &Path, position: Position) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.signature, "aide a la signature") else {
            return id;
        };
        self.position_request(id, index, position, "textDocument/signatureHelp", Kind::Signature);
        id
    }

    pub fn rename(&mut self, path: &Path, position: Position, new_name: &str) -> u64 {
        let id = self.take_id();
        if new_name.trim().is_empty() {
            return self.reject(id, "le nouveau nom est vide");
        }
        let Some(index) = self.prepare(id, path, self.caps.rename, "renommage") else {
            return id;
        };
        open_request(&mut self.payload, id, "textDocument/rename");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("},\"position\":");
        push_position(&mut self.payload, position);
        self.payload.push_str(",\"newName\":");
        push_text(&mut self.payload, new_name);
        self.payload.push_str("}}");
        self.dispatch();
        self.track(id, Kind::Rename, PathBuf::new());
        id
    }

    pub fn formatting(&mut self, path: &Path) -> u64 {
        let id = self.take_id();
        let Some(index) = self.prepare(id, path, self.caps.formatting, "formatage") else {
            return id;
        };
        open_request(&mut self.payload, id, "textDocument/formatting");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str(
            "},\"options\":{\"tabSize\":4,\"insertSpaces\":true,\"trimTrailingWhitespace\":true,\"insertFinalNewline\":true}}}",
        );
        self.dispatch();
        self.track(id, Kind::Formatting, PathBuf::new());
        id
    }

    pub fn cancel(&mut self, id: u64) {
        let Some(slot) = self.pending.iter().position(|entry| entry.id == id) else {
            return;
        };
        self.pending.swap_remove(slot);
        self.send_cancel(id);
    }

    pub fn stop(&mut self) {
        if !self.exited && self.ready {
            let id = self.take_id();
            self.payload.clear();
            self.payload.push_str("{\"jsonrpc\":\"2.0\",\"id\":");
            push_number(&mut self.payload, id);
            self.payload.push_str(",\"method\":\"shutdown\"}");
            let _ = self.transport.send(&self.payload);
            self.payload.clear();
            self.payload.push_str("{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}");
            let _ = self.transport.send(&self.payload);
        }
        self.exited = true;
        self.ready = false;
        self.pending.clear();
        self.queued.clear();
        self.docs.clear();
        self.progress.clear();
        self.status.clear();
        self.status.push_str("serveur arrete");
        self.transport.stop();
    }

    fn send_initialize(&mut self) {
        let id = self.take_id();
        let uri = uri_from_path(&self.root);
        open_request(&mut self.payload, id, "initialize");
        self.payload.push_str("{\"processId\":");
        push_number(&mut self.payload, std::process::id() as u64);
        self.payload.push_str(",\"clientInfo\":{\"name\":\"text3d\",\"version\":\"0.1.0\"},\"locale\":\"fr\",\"trace\":\"off\",\"initializationOptions\":{},\"rootPath\":");
        let root_text = self.root.to_string_lossy();
        push_text(&mut self.payload, &root_text);
        self.payload.push_str(",\"rootUri\":");
        push_text(&mut self.payload, &uri);
        self.payload.push_str(",\"workspaceFolders\":[{\"uri\":");
        push_text(&mut self.payload, &uri);
        self.payload.push_str(",\"name\":");
        let name = self.root.file_name().and_then(|value| value.to_str()).unwrap_or("projet");
        push_text(&mut self.payload, name);
        self.payload.push_str("}],\"capabilities\":");
        self.payload.push_str(CLIENT_CAPABILITIES);
        self.payload.push_str("}}");
        if let Err(error) = self.transport.send(&self.payload) {
            self.failure = Some(error);
        }
        self.track(id, Kind::Initialize, PathBuf::new());
    }

    fn prepare(&mut self, id: u64, path: &Path, supported: bool, what: &str) -> Option<usize> {
        let Some(index) = self.doc_index(path) else {
            self.reject(id, "fichier non ouvert dans le serveur de langage");
            return None;
        };
        if self.ready && !supported {
            self.line.clear();
            self.line.push_str(what);
            self.line.push_str(" non pris en charge par ce serveur");
            let message = self.line.clone();
            self.rejected.push((id, message));
            return None;
        }
        self.flush_doc(index);
        Some(index)
    }

    fn position_request(
        &mut self,
        id: u64,
        index: usize,
        position: Position,
        method: &str,
        kind: Kind,
    ) {
        open_request(&mut self.payload, id, method);
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str("},\"position\":");
        push_position(&mut self.payload, position);
        self.payload.push_str("}}");
        self.dispatch();
        self.track(id, kind, PathBuf::new());
    }

    fn reject(&mut self, id: u64, message: &str) -> u64 {
        self.rejected.push((id, message.to_string()));
        id
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn track(&mut self, id: u64, kind: Kind, path: PathBuf) {
        self.pending.push(Pending { id, kind, path, sent: Instant::now() });
    }

    fn doc_index(&self, path: &Path) -> Option<usize> {
        if let Some(index) = self.docs.iter().position(|doc| doc.path == path) {
            return Some(index);
        }
        let resolved = std::fs::canonicalize(path).ok()?;
        self.docs.iter().position(|doc| doc.path == resolved)
    }

    fn store_change(&mut self, index: usize, text: &str) {
        let doc = &mut self.docs[index];
        let fresh = doc.pending.is_none();
        let mut buffer = match doc.pending.take() {
            Some(buffer) => buffer,
            None => doc.spare.take().unwrap_or_default(),
        };
        buffer.clear();
        buffer.reserve(text.len());
        buffer.push_str(text);
        doc.pending = Some(buffer);
        if fresh {
            doc.since = Instant::now();
        }
    }

    fn flush_doc(&mut self, index: usize) {
        let Some(text) = self.docs[index].pending.take() else {
            return;
        };
        self.docs[index].version += 1;
        let version = self.docs[index].version;
        open_notification(&mut self.payload, "textDocument/didChange");
        self.payload.push_str("{\"textDocument\":{\"uri\":");
        push_text(&mut self.payload, &self.docs[index].uri);
        self.payload.push_str(",\"version\":");
        push_number(&mut self.payload, version as u64);
        self.payload.push_str("},\"contentChanges\":[{\"text\":");
        push_text(&mut self.payload, &text);
        self.payload.push_str("}]}}");
        self.dispatch();
        let mut spare = text;
        spare.clear();
        self.docs[index].spare = Some(spare);
    }

    fn dispatch(&mut self) {
        if !self.ready {
            self.queued.push(self.payload.clone());
            return;
        }
        if let Err(error) = self.transport.send(&self.payload)
            && self.failure.is_none() {
                self.failure = Some(error);
            }
    }

    fn send_cancel(&mut self, id: u64) {
        open_notification(&mut self.payload, "$/cancelRequest");
        self.payload.push_str("{\"id\":");
        push_number(&mut self.payload, id);
        self.payload.push_str("}}");
        self.dispatch();
    }

    fn collect_logs(&mut self) {
        self.drain.clear();
        self.transport.drain_stderr(&mut self.drain);
        if self.drain.is_empty() {
            return;
        }
        for line in self.drain.drain(..) {
            self.log.push(line);
        }
        if self.log.len() > MAX_LOG_LINES {
            let excess = self.log.len() - MAX_LOG_LINES;
            self.log.drain(..excess);
        }
    }

    fn handle(&mut self, message: &Json, out: &mut Vec<Event>) {
        if let Some(method) = message.get("method").and_then(Json::as_str) {
            match message.get("id") {
                Some(id) => self.answer(id, method, message),
                None => self.notify(method, message, out),
            }
            return;
        }
        self.respond(message, out);
    }

    fn answer(&mut self, id: &Json, method: &str, message: &Json) {
        self.payload.clear();
        self.payload.push_str("{\"jsonrpc\":\"2.0\",\"id\":");
        push_raw_id(&mut self.payload, id);
        match method {
            "workspace/configuration" => {
                let count = message
                    .path("params.items")
                    .and_then(Json::as_array)
                    .map(<[Json]>::len)
                    .unwrap_or(1);
                self.payload.push_str(",\"result\":[");
                for index in 0..count.max(1) {
                    if index > 0 {
                        self.payload.push(',');
                    }
                    self.payload.push_str("null");
                }
                self.payload.push(']');
            }
            "workspace/applyEdit" => self.payload.push_str(",\"result\":{\"applied\":false}"),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => self.payload.push_str(",\"result\":null"),
            _ => self
                .payload
                .push_str(",\"error\":{\"code\":-32601,\"message\":\"methode non prise en charge\"}"),
        }
        self.payload.push('}');
        if let Err(error) = self.transport.send(&self.payload)
            && self.failure.is_none() {
                self.failure = Some(error);
            }
    }

    fn notify(&mut self, method: &str, message: &Json, out: &mut Vec<Event>) {
        match method {
            "textDocument/publishDiagnostics" => {
                let params = message.get("params").unwrap_or(&NULL);
                let Some(uri) = params.get("uri").and_then(Json::as_str) else {
                    return;
                };
                let Some(path) = path_from_uri(uri) else {
                    return;
                };
                let mut items = Vec::new();
                parse_diagnostics(params.get("diagnostics").unwrap_or(&NULL), &mut items);
                out.push(Event::Diagnostics { path, items });
            }
            "$/progress" => self.progress(message, out),
            "window/logMessage" | "window/showMessage" => {
                let text = message.path("params.message").and_then(Json::as_str).unwrap_or("");
                if !text.is_empty() {
                    self.line.clear();
                    condense(text, &mut self.line);
                    assign_status(&mut self.status, &self.line, out);
                }
            }
            "experimental/serverStatus" => {
                let params = message.get("params").unwrap_or(&NULL);
                let quiet = params.get("quiescent").and_then(Json::as_bool).unwrap_or(false);
                let text = params.get("message").and_then(Json::as_str).unwrap_or("");
                self.line.clear();
                if !text.is_empty() {
                    condense(text, &mut self.line);
                } else if quiet {
                    self.line.push_str("indexation terminee");
                } else {
                    self.line.push_str("indexation en cours");
                }
                assign_status(&mut self.status, &self.line, out);
            }
            _ => {}
        }
    }

    fn progress(&mut self, message: &Json, out: &mut Vec<Event>) {
        let params = message.get("params").unwrap_or(&NULL);
        self.token.clear();
        match params.get("token") {
            Some(Json::String(text)) => self.token.push_str(text),
            Some(Json::Number(value)) => push_number(&mut self.token, *value as u64),
            _ => return,
        }
        let value = params.get("value").unwrap_or(&NULL);
        let stage = value.get("kind").and_then(Json::as_str).unwrap_or("");
        let title = value.get("title").and_then(Json::as_str).unwrap_or("");
        if stage == "begin" && self.progress.len() < MAX_PROGRESS {
            self.progress.push((self.token.clone(), title.to_string()));
        }
        self.line.clear();
        match self.progress.iter().find(|entry| entry.0 == self.token) {
            Some(entry) => condense(&entry.1, &mut self.line),
            None => condense(title, &mut self.line),
        }
        if let Some(text) = value.get("message").and_then(Json::as_str) {
            if !self.line.is_empty() {
                self.line.push(' ');
            }
            condense(text, &mut self.line);
        }
        if let Some(percent) = value.get("percentage").and_then(Json::as_u32) {
            if !self.line.is_empty() {
                self.line.push(' ');
            }
            push_number(&mut self.line, percent.min(100) as u64);
            self.line.push('%');
        }
        if stage == "end" {
            if let Some(slot) = self.progress.iter().position(|entry| entry.0 == self.token) {
                self.progress.swap_remove(slot);
            }
            if self.line.is_empty() {
                self.line.push_str("travail termine");
            }
        }
        if self.line.is_empty() {
            return;
        }
        assign_status(&mut self.status, &self.line, out);
    }

    fn respond(&mut self, message: &Json, out: &mut Vec<Event>) {
        let Some(raw) = message.get("id").and_then(Json::as_i64) else {
            return;
        };
        if raw < 0 {
            return;
        }
        let id = raw as u64;
        let Some(slot) = self.pending.iter().position(|entry| entry.id == id) else {
            return;
        };
        let entry = self.pending.swap_remove(slot);
        if let Some(error) = message.get("error") {
            let text = error.get("message").and_then(Json::as_str).unwrap_or("erreur du serveur");
            if entry.kind == Kind::Initialize {
                self.line.clear();
                self.line.push_str("initialisation refusee: ");
                condense(text, &mut self.line);
                let reason = self.line.clone();
                self.finish(reason, out);
                return;
            }
            self.line.clear();
            condense(text, &mut self.line);
            out.push(Event::Failed { id, message: self.line.clone() });
            return;
        }
        let result = message.get("result").unwrap_or(&NULL);
        match entry.kind {
            Kind::Initialize => self.accept(result, out),
            Kind::Shutdown => {}
            Kind::Completion => {
                let mut items = Vec::new();
                let incomplete = parse_completion(result, &mut items);
                out.push(Event::Completion { id, items, incomplete });
            }
            Kind::Hover => {
                parse_hover(result, &mut self.hover);
                out.push(Event::Hover { id, text: self.hover.clone() });
            }
            Kind::Definition => {
                let mut locations = Vec::new();
                parse_locations(result, &mut locations);
                out.push(Event::Definition { id, locations });
            }
            Kind::References => {
                let mut locations = Vec::new();
                parse_locations(result, &mut locations);
                out.push(Event::References { id, locations });
            }
            Kind::DocumentSymbols => {
                let mut items = Vec::new();
                parse_document_symbols(result, &entry.path, &mut items);
                out.push(Event::DocumentSymbols { id, items });
            }
            Kind::WorkspaceSymbols => {
                let mut items = Vec::new();
                parse_workspace_symbols(result, &mut items);
                out.push(Event::WorkspaceSymbols { id, items });
            }
            Kind::Signature => out.push(Event::Signature { id, info: parse_signature(result) }),
            Kind::Rename => {
                let mut edits = Vec::new();
                parse_workspace_edit(result, &mut edits);
                out.push(Event::Rename { id, edits });
            }
            Kind::Formatting => {
                let mut edits = Vec::new();
                parse_text_edits(result, &mut edits);
                out.push(Event::Formatting { id, edits });
            }
        }
    }

    fn accept(&mut self, result: &Json, out: &mut Vec<Event>) {
        let caps = result.get("capabilities").unwrap_or(&NULL);
        self.caps.completion = provider(caps, "completionProvider");
        self.caps.hover = provider(caps, "hoverProvider");
        self.caps.definition = provider(caps, "definitionProvider");
        self.caps.references = provider(caps, "referencesProvider");
        self.caps.document_symbol = provider(caps, "documentSymbolProvider");
        self.caps.workspace_symbol = provider(caps, "workspaceSymbolProvider");
        self.caps.signature = provider(caps, "signatureHelpProvider");
        self.caps.rename = provider(caps, "renameProvider");
        self.caps.formatting = provider(caps, "documentFormattingProvider");
        self.caps.save_text =
            caps.path("textDocumentSync.save.includeText").and_then(Json::as_bool).unwrap_or(false);
        self.payload.clear();
        self.payload.push_str("{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}");
        if let Err(error) = self.transport.send(&self.payload) {
            self.failure = Some(error);
            return;
        }
        self.ready = true;
        let queued = std::mem::take(&mut self.queued);
        for payload in &queued {
            if let Err(error) = self.transport.send(payload) {
                if self.failure.is_none() {
                    self.failure = Some(error);
                }
                break;
            }
        }
        self.queued = queued;
        self.queued.clear();
        out.push(Event::Ready);
        self.line.clear();
        self.line.push_str("serveur pret");
        assign_status(&mut self.status, &self.line, out);
    }

    fn check_timeouts(&mut self, out: &mut Vec<Event>) {
        if self.pending.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut index = 0;
        while index < self.pending.len() {
            if now.duration_since(self.pending[index].sent) < REQUEST_TIMEOUT {
                index += 1;
                continue;
            }
            let entry = self.pending.swap_remove(index);
            if entry.kind == Kind::Initialize {
                self.failure = Some("le serveur n a pas repondu a l initialisation".to_string());
                continue;
            }
            self.send_cancel(entry.id);
            out.push(Event::Failed {
                id: entry.id,
                message: "delai depasse, requete abandonnee".to_string(),
            });
        }
    }

    fn finish(&mut self, reason: String, out: &mut Vec<Event>) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.ready = false;
        self.collect_logs();
        for entry in self.pending.drain(..) {
            if entry.kind == Kind::Initialize {
                continue;
            }
            out.push(Event::Failed {
                id: entry.id,
                message: "le serveur de langage s est arrete".to_string(),
            });
        }
        let mut message = reason;
        let start = self.log.len().saturating_sub(EXIT_LOG_LINES);
        for line in &self.log[start..] {
            message.push('\n');
            message.push_str(line);
        }
        self.queued.clear();
        self.docs.clear();
        self.progress.clear();
        self.status.clear();
        self.status.push_str("serveur arrete");
        self.transport.stop();
        out.push(Event::Exited { message });
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn language_id(path: &Path, kind: ServerKind) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs") => "rust",
        Some("cs") | Some("csx") => "csharp",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("xml") | Some("csproj") | Some("props") | Some("targets") => "xml",
        _ => kind.fallback_language(),
    }
}

fn runnable(path: &Path) -> bool {
    let Ok(data) = std::fs::metadata(path) else {
        return false;
    };
    if !data.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        data.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn in_path(name: &str) -> Option<PathBuf> {
    let raw = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&raw) {
        let candidate = directory.join(name);
        if runnable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn provider(caps: &Json, name: &str) -> bool {
    match caps.get(name) {
        Some(Json::Bool(value)) => *value,
        Some(Json::Object(_)) => true,
        _ => false,
    }
}

fn condense(text: &str, out: &mut String) {
    let mut count = out.chars().count();
    let mut space = out.is_empty() || out.ends_with(' ');
    for character in text.chars() {
        if count >= MAX_STATUS {
            break;
        }
        if character.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
                count += 1;
            }
            continue;
        }
        space = false;
        for lowered in character.to_lowercase() {
            out.push(lowered);
            count += 1;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
}

fn assign_status(status: &mut String, text: &str, out: &mut Vec<Event>) {
    if status == text || text.is_empty() {
        return;
    }
    status.clear();
    status.push_str(text);
    out.push(Event::Status(status.clone()));
}

fn open_request(out: &mut String, id: u64, method: &str) {
    out.clear();
    out.push_str("{\"jsonrpc\":\"2.0\",\"id\":");
    push_number(out, id);
    out.push_str(",\"method\":");
    push_text(out, method);
    out.push_str(",\"params\":");
}

fn open_notification(out: &mut String, method: &str) {
    out.clear();
    out.push_str("{\"jsonrpc\":\"2.0\",\"method\":");
    push_text(out, method);
    out.push_str(",\"params\":");
}

fn push_number(out: &mut String, value: u64) {
    let mut digits = [0u8; 20];
    let mut cursor = digits.len();
    let mut left = value;
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&digits[cursor..]).unwrap_or("0"));
}

fn push_raw_id(out: &mut String, id: &Json) {
    match id {
        Json::Number(value) => {
            let number = *value as i64;
            if number < 0 {
                out.push('-');
            }
            push_number(out, number.unsigned_abs());
        }
        Json::String(text) => push_text(out, text),
        _ => out.push_str("null"),
    }
}

fn push_position(out: &mut String, position: Position) {
    out.push_str("{\"line\":");
    push_number(out, position.line as u64);
    out.push_str(",\"character\":");
    push_number(out, position.character as u64);
    out.push('}');
}

fn push_text(out: &mut String, value: &str) {
    out.reserve(value.len() + 2);
    out.push('"');
    let bytes = value.as_bytes();
    let mut start = 0;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if byte >= 0x20 && byte != b'"' && byte != b'\\' {
            continue;
        }
        if start < index {
            out.push_str(&value[start..index]);
        }
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            other => {
                out.push_str("\\u00");
                out.push(char::from(b"0123456789abcdef"[(other >> 4) as usize]));
                out.push(char::from(b"0123456789abcdef"[(other & 0x0f) as usize]));
            }
        }
        start = index + 1;
    }
    if start < bytes.len() {
        out.push_str(&value[start..]);
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Json {
        match Json::parse(text) {
            Ok(value) => value,
            Err(error) => panic!("json de test invalide: {error}"),
        }
    }

    #[test]
    fn echappement_des_chaines() {
        let mut out = String::new();
        push_text(&mut out, "a\"b\\c\nd\te\u{1}f");
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\te\\u0001f\"");
        out.clear();
        push_text(&mut out, "caf\u{e9} \u{1f600}");
        assert_eq!(out, "\"caf\u{e9} \u{1f600}\"");
        out.clear();
        push_text(&mut out, "");
        assert_eq!(out, "\"\"");
    }

    #[test]
    fn echappement_relu_par_l_analyseur() {
        let source = "ligne1\nligne2 \"guillemets\" \\ backslash \u{7} \u{e9} \u{1f680}";
        let mut out = String::from("{\"texte\":");
        push_text(&mut out, source);
        out.push('}');
        let value = json(&out);
        assert_eq!(value.get("texte").and_then(Json::as_str), Some(source));
    }

    #[test]
    fn nombres_ecrits_sans_allocation() {
        let mut out = String::new();
        push_number(&mut out, 0);
        push_number(&mut out, 7);
        push_number(&mut out, 1234567890);
        assert_eq!(out, "071234567890");
        out.clear();
        push_number(&mut out, u64::MAX);
        assert_eq!(out, "18446744073709551615");
    }

    #[test]
    fn identifiant_brut_recopie() {
        let mut out = String::new();
        push_raw_id(&mut out, &json("12"));
        out.push(' ');
        push_raw_id(&mut out, &json("\"abc\""));
        out.push(' ');
        push_raw_id(&mut out, &json("null"));
        out.push(' ');
        push_raw_id(&mut out, &json("-4"));
        assert_eq!(out, "12 \"abc\" null -4");
    }

    #[test]
    fn requete_bien_formee() {
        let mut out = String::new();
        open_request(&mut out, 42, "textDocument/hover");
        out.push_str("{\"textDocument\":{\"uri\":\"file:///tmp/a.rs\"},\"position\":");
        push_position(&mut out, Position::new(3, 12));
        out.push_str("}}");
        let value = json(&out);
        assert_eq!(value.get("id").and_then(Json::as_i64), Some(42));
        assert_eq!(value.get("method").and_then(Json::as_str), Some("textDocument/hover"));
        assert_eq!(value.path("params.position.character").and_then(Json::as_u32), Some(12));
    }

    #[test]
    fn notification_bien_formee() {
        let mut out = String::new();
        open_notification(&mut out, "textDocument/didChange");
        out.push_str("{\"textDocument\":{\"uri\":\"file:///tmp/a.rs\",\"version\":3},\"contentChanges\":[{\"text\":");
        push_text(&mut out, "fn f() {}\n");
        out.push_str("}]}}");
        let value = json(&out);
        assert!(value.get("id").is_none());
        assert_eq!(value.path("params.contentChanges.0.text").and_then(Json::as_str), Some("fn f() {}\n"));
    }

    #[test]
    fn capacites_client_valides() {
        let value = json(CLIENT_CAPABILITIES);
        assert_eq!(
            value.path("textDocument.completion.completionItem.snippetSupport").and_then(Json::as_bool),
            Some(false)
        );
        assert!(value.path("textDocument.completion.completionItem.resolveSupport").is_none());
        assert_eq!(value.path("window.workDoneProgress").and_then(Json::as_bool), Some(true));
        assert_eq!(value.path("textDocument.definition.linkSupport").and_then(Json::as_bool), Some(true));
        assert_eq!(
            value.path("textDocument.documentSymbol.hierarchicalDocumentSymbolSupport").and_then(Json::as_bool),
            Some(true)
        );
        assert_eq!(value.path("general.positionEncodings.0").and_then(Json::as_str), Some("utf-16"));
    }

    #[test]
    fn fournisseurs_lus_des_capacites() {
        let caps = json("{\"hoverProvider\":true,\"completionProvider\":{\"triggerCharacters\":[\".\"]},\"renameProvider\":false}");
        assert!(provider(&caps, "hoverProvider"));
        assert!(provider(&caps, "completionProvider"));
        assert!(!provider(&caps, "renameProvider"));
        assert!(!provider(&caps, "absentProvider"));
    }

    #[test]
    fn statut_condense_et_dedoublonne() {
        let mut line = String::new();
        condense("  Indexing   Crates \n 42 ", &mut line);
        assert_eq!(line, "indexing crates 42");
        let mut status = String::new();
        let mut out = Vec::new();
        assign_status(&mut status, &line, &mut out);
        assign_status(&mut status, &line, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(status, "indexing crates 42");
        let mut long = String::new();
        condense(&"mot ".repeat(200), &mut long);
        assert!(long.chars().count() <= MAX_STATUS);
    }

    #[test]
    fn langage_deduit_de_l_extension() {
        assert_eq!(language_id(Path::new("/a/b.rs"), ServerKind::CSharp), "rust");
        assert_eq!(language_id(Path::new("/a/b.cs"), ServerKind::Rust), "csharp");
        assert_eq!(language_id(Path::new("/a/Cargo.toml"), ServerKind::Rust), "toml");
        assert_eq!(language_id(Path::new("/a/inconnu"), ServerKind::Rust), "rust");
        assert_eq!(language_id(Path::new("/a/inconnu"), ServerKind::CSharp), "csharp");
    }

    #[test]
    fn localisation_des_serveurs_sans_panique() {
        let rust = Server::locate(ServerKind::Rust);
        let sharp = Server::locate(ServerKind::CSharp);
        for found in [rust, sharp].into_iter().flatten() {
            assert!(found.is_absolute(), "chemin non absolu: {}", found.display());
            assert!(runnable(&found), "binaire non executable: {}", found.display());
        }
    }

    #[test]
    fn racine_invalide_refusee() {
        let result = Server::start(ServerKind::Rust, Path::new("/chemin/qui/n/existe/pas"));
        match result {
            Ok(_) => panic!("une racine inexistante ne doit pas demarrer"),
            Err(message) => assert!(
                message.contains("illisible") || message.contains("introuvable"),
                "message inattendu: {message}"
            ),
        }
    }

    #[test]
    fn libelles_des_serveurs() {
        assert_eq!(ServerKind::Rust.label(), "rust-analyzer");
        assert_eq!(ServerKind::CSharp.label(), "csharp-ls");
    }
}
