use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::json::Json;

const MAX_BODY: usize = 256 * 1024 * 1024;
const MAX_LINE: usize = 8 * 1024;
const READ_BUFFER: usize = 64 * 1024;
const STOP_GRACE: Duration = Duration::from_millis(1200);
const STOP_STEP: Duration = Duration::from_millis(5);

pub struct Transport {
    child: Child,
    input: Option<ChildStdin>,
    messages: Receiver<Json>,
    logs: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    logger: Option<JoinHandle<()>>,
    frame: Vec<u8>,
    closed: bool,
    stopped: bool,
}

impl Transport {
    pub fn spawn(program: &Path, args: &[String], root: &Path) -> Result<Transport, String> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("lancement de {} impossible: {error}", program.display()))?;
        let taken = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(input), Some(output), Some(errors)) = taken else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("tubes du serveur de langage indisponibles".to_string());
        };
        let (message_sender, messages) = mpsc::channel();
        let (log_sender, logs) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("lsp-lecture".to_string())
            .spawn(move || {
                let mut source = BufReader::with_capacity(READ_BUFFER, output);
                read_frames(&mut source, &message_sender);
            });
        let reader = match reader {
            Ok(handle) => handle,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("thread de lecture impossible: {error}"));
            }
        };
        let logger = thread::Builder::new()
            .name("lsp-journal".to_string())
            .spawn(move || {
                let mut source = BufReader::with_capacity(MAX_LINE, errors);
                read_lines(&mut source, &log_sender);
            });
        let logger = match logger {
            Ok(handle) => handle,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("thread de journal impossible: {error}"));
            }
        };
        Ok(Transport {
            child,
            input: Some(input),
            messages,
            logs,
            reader: Some(reader),
            logger: Some(logger),
            frame: Vec::with_capacity(READ_BUFFER),
            closed: false,
            stopped: false,
        })
    }

    pub fn send(&mut self, payload: &str) -> Result<(), String> {
        let Some(input) = self.input.as_mut() else {
            return Err("serveur de langage arrete".to_string());
        };
        self.frame.clear();
        self.frame.reserve(payload.len() + 32);
        self.frame.extend_from_slice(b"Content-Length: ");
        let mut digits = [0u8; 20];
        let mut cursor = digits.len();
        let mut left = payload.len();
        loop {
            cursor -= 1;
            digits[cursor] = b'0' + (left % 10) as u8;
            left /= 10;
            if left == 0 {
                break;
            }
        }
        self.frame.extend_from_slice(&digits[cursor..]);
        self.frame.extend_from_slice(b"\r\n\r\n");
        self.frame.extend_from_slice(payload.as_bytes());
        match input.write_all(&self.frame).and_then(|()| input.flush()) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.closed = true;
                Err(format!("ecriture vers le serveur impossible: {error}"))
            }
        }
    }

    pub fn poll(&mut self) -> Option<Json> {
        match self.messages.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
        }
    }

    pub fn drain_stderr(&mut self, out: &mut Vec<String>) {
        while let Ok(line) = self.logs.try_recv() {
            out.push(line);
        }
    }

    pub fn alive(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        if !self.closed {
            return true;
        }
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.input = None;
        let deadline = Instant::now() + STOP_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    thread::sleep(STOP_STEP);
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.logger.take() {
            let _ = handle.join();
        }
        self.closed = true;
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_capped_line<R: BufRead>(source: &mut R, out: &mut Vec<u8>) -> bool {
    out.clear();
    loop {
        let chunk = match source.fill_buf() {
            Ok(chunk) => chunk,
            Err(_) => return false,
        };
        if chunk.is_empty() {
            return !out.is_empty();
        }
        match chunk.iter().position(|&byte| byte == b'\n') {
            Some(index) => {
                copy_capped(&chunk[..index], out);
                source.consume(index + 1);
                return true;
            }
            None => {
                let taken = chunk.len();
                copy_capped(chunk, out);
                source.consume(taken);
            }
        }
    }
}

fn copy_capped(chunk: &[u8], out: &mut Vec<u8>) {
    if out.len() >= MAX_LINE {
        return;
    }
    let room = MAX_LINE - out.len();
    out.extend_from_slice(&chunk[..chunk.len().min(room)]);
}

fn trim_return(out: &mut Vec<u8>) {
    while out.last() == Some(&b'\r') {
        out.pop();
    }
}

fn header_length(line: &[u8]) -> Option<usize> {
    let colon = line.iter().position(|&byte| byte == b':')?;
    let name = &line[..colon];
    if !name.eq_ignore_ascii_case(b"content-length") {
        return None;
    }
    let mut value = 0usize;
    let mut seen = false;
    for &byte in &line[colon + 1..] {
        if byte == b' ' || byte == b'\t' {
            if seen {
                break;
            }
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        seen = true;
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    if seen { Some(value) } else { None }
}

fn read_frames<R: BufRead>(source: &mut R, sender: &Sender<Json>) {
    let mut header = Vec::with_capacity(128);
    let mut body = Vec::with_capacity(READ_BUFFER);
    loop {
        let mut length = None;
        loop {
            if !read_capped_line(source, &mut header) {
                return;
            }
            trim_return(&mut header);
            if header.is_empty() {
                break;
            }
            if let Some(value) = header_length(&header) {
                length = Some(value);
            }
        }
        let Some(size) = length else {
            return;
        };
        if size > MAX_BODY {
            return;
        }
        body.clear();
        body.resize(size, 0);
        if source.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(text) = std::str::from_utf8(&body) else {
            continue;
        };
        let Ok(value) = Json::parse(text) else {
            continue;
        };
        if sender.send(value).is_err() {
            return;
        }
    }
}

fn read_lines<R: BufRead>(source: &mut R, sender: &Sender<String>) {
    let mut line = Vec::with_capacity(256);
    while read_capped_line(source, &mut line) {
        trim_return(&mut line);
        if line.is_empty() {
            continue;
        }
        if sender.send(String::from_utf8_lossy(&line).into_owned()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(payload: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{payload}", payload.len())
    }

    fn collect(input: &str) -> Vec<Json> {
        let (sender, receiver) = mpsc::channel();
        let mut source = Cursor::new(input.as_bytes().to_vec());
        read_frames(&mut source, &sender);
        drop(sender);
        receiver.into_iter().collect()
    }

    #[test]
    fn cadres_multiples_lus_a_la_suite() {
        let mut input = frame("{\"id\":1,\"result\":null}");
        input.push_str(&frame("{\"id\":2,\"result\":{\"a\":[1,2,3]}}"));
        let out = collect(&input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].get("id").and_then(Json::as_i64), Some(1));
        assert_eq!(out[1].path("result.a.2").and_then(Json::as_i64), Some(3));
    }

    #[test]
    fn entete_content_type_ignore_et_casse_libre() {
        let payload = "{\"method\":\"salut\"}";
        let input = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{payload}",
            payload.len()
        );
        let out = collect(&input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("method").and_then(Json::as_str), Some("salut"));
    }

    #[test]
    fn corps_tronque_ne_panique_pas() {
        let input = "Content-Length: 40\r\n\r\n{\"id\":1}";
        assert!(collect(input).is_empty());
    }

    #[test]
    fn entete_sans_longueur_arrete_la_lecture() {
        let input = "Content-Type: texte\r\n\r\n{\"id\":1}";
        assert!(collect(input).is_empty());
    }

    #[test]
    fn corps_json_invalide_ignore_et_suite_lue() {
        let mut input = frame("{ceci n est pas du json}");
        input.push_str(&frame("{\"ok\":true}"));
        let out = collect(&input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("ok").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn corps_volumineux_lu_entier() {
        let mut payload = String::from("{\"items\":[");
        for index in 0..40000 {
            if index > 0 {
                payload.push(',');
            }
            payload.push_str("\"element\"");
        }
        payload.push_str("]}");
        assert!(payload.len() > 400_000);
        let out = collect(&frame(&payload));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("items").and_then(Json::as_array).map(<[Json]>::len), Some(40000));
    }

    #[test]
    fn utf8_multioctet_traverse_le_cadre() {
        let payload = "{\"texte\":\"caf\u{e9} \u{1f600} \u{4e2d}\"}";
        let out = collect(&frame(payload));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("texte").and_then(Json::as_str), Some("caf\u{e9} \u{1f600} \u{4e2d}"));
    }

    #[test]
    fn lignes_de_journal_decoupees_et_plafonnees() {
        let mut input = String::from("premiere ligne\nseconde ligne\r\n");
        input.push_str(&"x".repeat(MAX_LINE * 2));
        input.push('\n');
        let (sender, receiver) = mpsc::channel();
        let mut source = Cursor::new(input.into_bytes());
        read_lines(&mut source, &sender);
        drop(sender);
        let lines: Vec<String> = receiver.into_iter().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "premiere ligne");
        assert_eq!(lines[1], "seconde ligne");
        assert_eq!(lines[2].len(), MAX_LINE);
    }

    #[test]
    fn longueur_entete_analysee() {
        assert_eq!(header_length(b"Content-Length: 42"), Some(42));
        assert_eq!(header_length(b"content-length:7"), Some(7));
        assert_eq!(header_length(b"Content-Type: x"), None);
        assert_eq!(header_length(b"Content-Length: abc"), None);
        assert_eq!(header_length(b"sans deux points"), None);
    }

    #[test]
    fn programme_absent_rend_une_erreur() {
        let result = Transport::spawn(
            Path::new("/usr/bin/programme-qui-n-existe-pas-du-tout"),
            &[],
            Path::new("/tmp"),
        );
        assert!(result.is_err());
        let message = result.err().unwrap_or_default();
        assert!(message.starts_with("lancement de"), "message inattendu: {message}");
    }

    #[test]
    fn processus_mort_detecte_et_arret_propre() {
        let mut transport = match Transport::spawn(Path::new("/bin/echo"), &[], Path::new("/tmp")) {
            Ok(transport) => transport,
            Err(error) => panic!("echo introuvable: {error}"),
        };
        let mut attempts = 0;
        while transport.alive() && attempts < 400 {
            let _ = transport.poll();
            thread::sleep(Duration::from_millis(5));
            attempts += 1;
        }
        assert!(!transport.alive(), "le processus aurait du sortir");
        transport.stop();
        transport.stop();
        assert!(!transport.alive());
    }

    #[test]
    fn envoi_apres_arret_rend_une_erreur() {
        let mut transport = match Transport::spawn(Path::new("/bin/cat"), &[], Path::new("/tmp")) {
            Ok(transport) => transport,
            Err(error) => panic!("cat introuvable: {error}"),
        };
        transport.stop();
        assert!(transport.send("{\"a\":1}").is_err());
        assert!(transport.poll().is_none());
        let mut lines = Vec::new();
        transport.drain_stderr(&mut lines);
    }
}
