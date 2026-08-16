//! `trust-lsp` — a language server for Trust, over stdio.
//!
//! It reports diagnostics and nothing else, which is the whole of what this
//! compiler can currently say about a place in a file: a `SyntaxError` carries
//! a **line** and a message, and the lexer carries a line per token. There are
//! no columns and no spans anywhere, so a squiggle is a whole line, and hover,
//! go-to-definition and completion are not merely unwritten — they have
//! nothing to be built out of yet. See `docs/status.md`.
//!
//! What it does do is worth having on its own: every error the frontend can
//! find, as you type, from exactly the compiler that will build the program.
//! There is no second implementation of the language here to drift.

mod json;

use std::collections::BTreeMap;
use std::io::{Read, Write};

use json::Json;
use trustc::lang;

fn main() {
    let mut server = Server::default();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    while let Some(msg) = read_message(&mut input) {
        let Some(request) = json::parse(&msg) else {
            continue;
        };
        if server.handle(&request) {
            return;
        }
    }
}

/// What the editor has open, by URI.
#[derive(Default)]
struct Server {
    open: BTreeMap<String, String>,
}

impl Server {
    /// Handle one message. Returns true when the client has asked to stop.
    fn handle(&mut self, m: &Json) -> bool {
        let Some(method) = m.str_at(&["method"]) else {
            // A response to something we sent; nothing here sends requests.
            return false;
        };
        match method {
            "initialize" => {
                // Full text on every change: this server re-checks a whole
                // file anyway, so incremental sync would be bookkeeping for
                // no gain.
                reply(
                    m.get(&["id"]),
                    "{\"capabilities\":{\"textDocumentSync\":1},\
                       \"serverInfo\":{\"name\":\"trust-lsp\"}}",
                );
            }
            "shutdown" => reply(m.get(&["id"]), "null"),
            "exit" => return true,
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    m.str_at(&["params", "textDocument", "uri"]),
                    m.str_at(&["params", "textDocument", "text"]),
                ) {
                    self.open.insert(uri.to_string(), text.to_string());
                    self.check(uri);
                }
            }
            "textDocument/didChange" => {
                // With full sync there is exactly one change and it is the
                // whole document.
                if let (Some(uri), Some(changes)) = (
                    m.str_at(&["params", "textDocument", "uri"]),
                    m.arr_at(&["params", "contentChanges"]),
                ) && let Some(text) = changes.last().and_then(|c| c.str_at(&["text"]))
                {
                    self.open.insert(uri.to_string(), text.to_string());
                    self.check(uri);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = m.str_at(&["params", "textDocument", "uri"]) {
                    self.open.remove(uri);
                    // An editor keeps what it was last told, so a file being
                    // closed has to be told it has nothing wrong with it.
                    publish(uri, &[]);
                }
            }
            _ => {
                // A request we do not implement still needs an answer, or the
                // client waits for one forever.
                if m.get(&["id"]).is_some() {
                    reply(m.get(&["id"]), "null");
                }
            }
        }
        false
    }

    /// Compile what is open at `uri` and publish what is wrong with it.
    fn check(&self, uri: &str) {
        let Some(src) = self.open.get(uri) else { return };
        let found = match lang::compile(src) {
            Ok(_) => Vec::new(),
            Err(errs) => errs,
        };
        let lines: Vec<&str> = src.lines().collect();
        let diagnostics: Vec<(usize, usize, &str)> = found
            .iter()
            .map(|e| {
                // Ours are 1-based and the protocol's are 0-based, and a
                // whole line is the only range there is anything to say.
                let line = e.line.saturating_sub(1) as usize;
                let end = lines.get(line).map_or(0, |l| l.chars().count());
                (line, end, e.message.as_str())
            })
            .collect();
        publish(uri, &diagnostics);
    }
}

/// Send a reply to a request that had an id.
fn reply(id: Option<&Json>, result: &str) {
    let id = match id {
        Some(Json::Num(n)) => format!("{}", *n as i64),
        Some(Json::Str(s)) => {
            let mut out = String::new();
            json::quote(s, &mut out);
            out
        }
        _ => return,
    };
    send(&format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"
    ));
}

/// Send the diagnostics for one file, which replaces whatever was sent last.
fn publish(uri: &str, found: &[(usize, usize, &str)]) {
    let mut items = String::new();
    for (i, (line, end, message)) in found.iter().enumerate() {
        if i > 0 {
            items.push(',');
        }
        let mut text = String::new();
        json::quote(message, &mut text);
        items.push_str(&format!(
            "{{\"range\":{{\"start\":{{\"line\":{line},\"character\":0}},\
               \"end\":{{\"line\":{line},\"character\":{end}}}}},\
               \"severity\":1,\"source\":\"trust\",\"message\":{text}}}"
        ));
    }
    let mut quoted = String::new();
    json::quote(uri, &mut quoted);
    send(&format!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\
           \"params\":{{\"uri\":{quoted},\"diagnostics\":[{items}]}}}}"
    ));
}

/// Write one message with the header the protocol frames them by.
fn send(body: &str) {
    let out = std::io::stdout();
    let mut out = out.lock();
    // `Content-Length` counts bytes, not characters, and a diagnostic may
    // quote a program's own text.
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

/// Read one message, or `None` at the end of the stream.
fn read_message(input: &mut impl Read) -> Option<String> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    // Headers, to the blank line.
    loop {
        if input.read(&mut byte).ok()? == 0 {
            return None;
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8(header).ok()?;
    let length: usize = header
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))?
        .trim()
        .parse()
        .ok()?;
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framed_message_is_read_back() {
        let mut input = "Content-Length: 17\r\n\r\n{\"method\":\"hi\"}\r\n".as_bytes();
        let msg = read_message(&mut input).expect("a message");
        assert_eq!(msg, "{\"method\":\"hi\"}\r\n");
    }

    #[test]
    fn a_file_with_an_error_produces_one_diagnostic_on_its_line() {
        let mut s = Server::default();
        s.open.insert(
            "file:///a.tr".into(),
            "fn main() -> t27 {\n    let x: t9 = 5;\n    x\n}\n".into(),
        );
        // The body's type is wrong, on the line the body's tail is on.
        let errs = lang::compile(&s.open["file:///a.tr"]).expect_err("an error");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("expected t27"), "{errs:?}");
        // 1-based here, 0-based there.
        assert!(errs[0].line >= 1);
    }
}
