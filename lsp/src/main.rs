//! `trust-lsp` — a language server for Trust, over stdio.
//!
//! Everything it answers is the compiler answering, not this:
//!
//!   * **diagnostics**, from `lang::compile` — every error the frontend can
//!     find, as you type, from exactly the compiler that will build the
//!     program. There is no second implementation of the language here to
//!     drift.
//!   * **an outline**, **go-to-definition**, **hover**, **find references**
//!     and **rename**, from `lang::index` — which reads the AST, so all of
//!     them work on a file that does not compile, which is the state a file
//!     is in while it is being written.
//!
//! This crate is the translation and nothing else: character offsets to the
//! protocol's UTF-16 columns, and structs to JSON.
//!
//!   * **completion**, from what is in scope, the prelude, and the keywords.
//!
//! A rename that cannot be trusted is refused with its reason rather than
//! done partly — `NoRename` lists the three ways that happens.
//!
//! What is not here is **member** completion. Which fields and methods
//! follow a `.` depends on the type of what precedes it, and a type is what
//! only lowering computes; so after a dot this offers **nothing** rather
//! than the names in scope, which are the one thing that certainly cannot
//! follow one. See `docs/spec-gaps.md` G0.19.

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
                    "{\"capabilities\":{\"textDocumentSync\":1,\
                       \"documentSymbolProvider\":true,\
                       \"definitionProvider\":true,\
                       \"hoverProvider\":true,\
                       \"referencesProvider\":true,\
                       \"renameProvider\":{\"prepareProvider\":true},\
                       \"completionProvider\":{}},\
                       \"serverInfo\":{\"name\":\"trust-lsp\"}}",
                );
            }
            "textDocument/documentSymbol" => {
                let out = self
                    .indexed(m)
                    .map(|(map, index, _)| symbols(&map, &index.symbols))
                    .unwrap_or_else(|| "[]".to_string());
                reply(m.get(&["id"]), &out);
            }
            "textDocument/hover" => {
                let out = self
                    .indexed(m)
                    .and_then(|(map, index, _)| {
                        let at = position(m, &map)?;
                        let (word, to) = index.use_at(at)?;
                        let def = index.describe(to)?;
                        // The definition as it was written, and then what it
                        // turned out to be — which is the same thing when the
                        // file said it, and the interesting part when it did
                        // not. `let n = 1` reads `let n` and is a `t27`.
                        let src = self
                            .open
                            .get(m.str_at(&["params", "textDocument", "uri"])?)?;
                        let found = lang::analyze(src).types;
                        let text = match found.exact(to).or_else(|| found.at(at)) {
                            Some(ty) if !def.label.ends_with(ty) => {
                                format!("{}\n// {ty}", def.label)
                            }
                            _ => def.label.clone(),
                        };
                        Some(hover(&map, word, &text))
                    })
                    .unwrap_or_else(|| "null".to_string());
                reply(m.get(&["id"]), &out);
            }
            "textDocument/completion" => {
                let out = self
                    .completions(m)
                    .unwrap_or_else(|| "{\"isIncomplete\":false,\"items\":[]}".to_string());
                reply(m.get(&["id"]), &out);
            }
            "textDocument/references" => {
                let out = self
                    .indexed(m)
                    .and_then(|(map, index, uri)| {
                        let at = position(m, &map)?;
                        let to = index.definition_at(at)?;
                        let places = index.references(to);
                        Some(list(places.iter().map(|s| location(&map, &uri, *s))))
                    })
                    .unwrap_or_else(|| "[]".to_string());
                reply(m.get(&["id"]), &out);
            }
            "textDocument/prepareRename" => {
                // The name under the cursor, if renaming it is something
                // this can do. A null answer is how an editor is told to say
                // "cannot rename here" before asking for a new name.
                let out = self
                    .indexed(m)
                    .and_then(|(map, index, _)| {
                        let at = position(m, &map)?;
                        let (word, _) = index.use_at(at)?;
                        // Any name will do to find out whether the *place*
                        // can be renamed; the real one is checked later.
                        match index.rename(at, "x") {
                            Err(
                                lang::index::NoRename::Method | lang::index::NoRename::NotAName,
                            ) => None,
                            _ => Some(range(&map, word)),
                        }
                    })
                    .unwrap_or_else(|| "null".to_string());
                reply(m.get(&["id"]), &out);
            }
            "textDocument/rename" => {
                let new = m.str_at(&["params", "newName"]).unwrap_or_default();
                match self.indexed(m) {
                    Some((map, index, uri)) => {
                        let places = position(m, &map).ok_or(lang::index::NoRename::NotAName);
                        match places.and_then(|at| index.rename(at, new)) {
                            Ok(spans) => {
                                let edits = list(spans.iter().map(|s| {
                                    let mut text = String::new();
                                    json::quote(new, &mut text);
                                    format!("{{\"range\":{},\"newText\":{text}}}", range(&map, *s))
                                }));
                                let mut quoted = String::new();
                                json::quote(&uri, &mut quoted);
                                reply(
                                    m.get(&["id"]),
                                    &format!("{{\"changes\":{{{quoted}:{edits}}}}}"),
                                );
                            }
                            // A refusal is an error and not an empty edit:
                            // an editor that is told "nothing changed" says
                            // nothing, and the reason is the useful part.
                            Err(why) => fail(m.get(&["id"]), &why.to_string()),
                        }
                    }
                    None => fail(m.get(&["id"]), "this file does not parse"),
                }
            }
            "textDocument/definition" => {
                let out = self
                    .indexed(m)
                    .and_then(|(map, index, uri)| {
                        let at = position(m, &map)?;
                        let to = index.definition_at(at)?;
                        Some(location(&map, &uri, to))
                    })
                    .unwrap_or_else(|| "null".to_string());
                reply(m.get(&["id"]), &out);
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

    /// The index of whatever file a request is about.
    ///
    /// Parsed with recovery, because the file is being typed into: an item
    /// that does not parse yet costs its own outline entry and nothing else.
    fn indexed(&self, m: &Json) -> Option<(lang::LineMap, lang::index::Index, String)> {
        let uri = m.str_at(&["params", "textDocument", "uri"])?;
        let src = self.open.get(uri)?;
        let (file, _) = lang::parse::parse_recovering(src);
        Some((
            lang::LineMap::new(src),
            lang::index::Index::new(&file),
            uri.to_string(),
        ))
    }

    /// What can be named where the cursor is.
    ///
    /// Three sources, and none of them needs a type: what is in scope in this
    /// file, what the prelude defines, and the keywords. After a `.` there is
    /// a fourth that this cannot supply — which member is there depends on
    /// the type of what precedes it — so it offers **nothing** rather than
    /// offering the locals, which are the one thing that certainly cannot
    /// follow a dot.
    fn completions(&self, m: &Json) -> Option<String> {
        let uri = m.str_at(&["params", "textDocument", "uri"])?;
        let src = self.open.get(uri)?;
        let map = lang::LineMap::new(src);
        let at = position(m, &map)?;
        let chars: Vec<char> = src.chars().collect();
        if let Some(dot) = dot_before(&chars, at) {
            return Some(self.members(src, &chars, dot));
        }
        let (file, _) = lang::parse::parse_recovering(src);
        let index = lang::index::Index::new(&file);

        let mut items: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for d in index.visible(at) {
            seen.push(d.name.clone());
            items.push(item(&d.name, completion_kind(d.kind), &d.label));
        }
        for d in lang::index::prelude().globals() {
            if !seen.contains(&d.name) {
                seen.push(d.name.clone());
                items.push(item(&d.name, completion_kind(d.kind), &d.label));
            }
        }
        for kw in lang::lex::KEYWORDS {
            items.push(item(kw, 14, "keyword"));
        }
        Some(format!(
            "{{\"isIncomplete\":false,\"items\":{}}}",
            list(items.into_iter())
        ))
    }

    /// What can be written after the `.` at `dot`.
    ///
    /// The receiver's type is what decides, and `p.` with nothing after it is
    /// not an expression — so the statement it sits in does not parse, and
    /// there is no type to be had. A name is written in after the dot, which
    /// makes it one. Everything **before** the dot keeps its offset, and the
    /// receiver is the only thing asked about: writing after it cannot change
    /// what it is.
    fn members(&self, src: &str, chars: &[char], dot: u32) -> String {
        let empty = "{\"isIncomplete\":false,\"items\":[]}".to_string();
        let mut probe: String = chars[..dot as usize + 1].iter().collect();
        probe.push_str("probe");
        let mut after = dot as usize + 1;
        while after < chars.len() && (chars[after].is_ascii_alphanumeric() || chars[after] == '_') {
            after += 1;
        }
        probe.extend(chars[after..].iter());

        // The last character of the receiver, which is what has a type.
        let Some(last) = dot.checked_sub(1) else {
            return empty;
        };
        let Some(ty) = lang::analyze(&probe).types.at(last).map(str::to_string) else {
            return empty;
        };
        let (file, _) = lang::parse::parse_recovering(src);
        let index = lang::index::Index::new(&file);

        let mut items = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for d in index
            .members(&ty)
            .into_iter()
            .chain(lang::index::prelude().members(&ty))
        {
            if !seen.contains(&d.name) {
                seen.push(d.name.clone());
                items.push(item(&d.name, completion_kind(d.kind), &d.label));
            }
        }
        for (name, sig) in lang::index::builtin_members(&ty) {
            if !seen.iter().any(|s| s == name) {
                seen.push(name.to_string());
                items.push(item(name, 2, sig));
            }
        }
        format!(
            "{{\"isIncomplete\":false,\"items\":{}}}",
            list(items.into_iter())
        )
    }

    /// Compile what is open at `uri` and publish what is wrong with it.
    fn check(&self, uri: &str) {
        let Some(src) = self.open.get(uri) else {
            return;
        };
        let found = lang::analyze(src).errors;
        let map = lang::LineMap::new(src);
        let diagnostics: Vec<Diagnostic<'_>> = found.iter().map(|e| at(&map, e)).collect();
        publish(uri, &diagnostics);
    }
}

/// The character offset a request's `position` names.
///
/// The protocol counts a column in UTF-16 code units and this compiler counts
/// characters, so the line is walked rather than added to.
fn position(m: &Json, map: &lang::LineMap) -> Option<u32> {
    let line = match m.get(&["params", "position", "line"])? {
        json::Json::Num(n) => *n as u32 + 1,
        _ => return None,
    };
    let utf16 = match m.get(&["params", "position", "character"])? {
        json::Json::Num(n) => *n as u32,
        _ => return None,
    };
    map.offset_of(line, utf16)
}

/// A span, as a range in the protocol's coordinates.
fn range(map: &lang::LineMap, span: lang::Span) -> String {
    let lo = map.pos(span.lo);
    let hi = map.pos(span.hi);
    format!(
        "{{\"start\":{{\"line\":{},\"character\":{}}},\
           \"end\":{{\"line\":{},\"character\":{}}}}}",
        lo.line - 1,
        lo.utf16,
        hi.line - 1,
        hi.utf16
    )
}

/// One place in one file.
fn location(map: &lang::LineMap, uri: &str, span: lang::Span) -> String {
    let mut quoted = String::new();
    json::quote(uri, &mut quoted);
    format!("{{\"uri\":{quoted},\"range\":{}}}", range(map, span))
}

/// Where the `.` is, when the cursor sits after one that opens a field or
/// a method access.
///
/// A range is written `a..b`, and the name after it is an ordinary one — so
/// two dots are not one. Nothing else in this language puts a dot before a
/// name (Ch. 0 §2.6).
fn dot_before(chars: &[char], at: u32) -> Option<u32> {
    let mut i = (at as usize).min(chars.len());
    while i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_') {
        i -= 1;
    }
    let dot = i.checked_sub(1)?;
    (chars[dot] == '.' && !(dot > 0 && chars[dot - 1] == '.')).then_some(dot as u32)
}

/// One thing a completion offers.
fn item(name: &str, kind: u32, detail: &str) -> String {
    let (mut label, mut d) = (String::new(), String::new());
    json::quote(name, &mut label);
    json::quote(detail, &mut d);
    format!("{{\"label\":{label},\"kind\":{kind},\"detail\":{d}}}")
}

/// The protocol's numbering for what a completion offers, which is a
/// different list from the one an outline uses.
fn completion_kind(k: lang::index::SymbolKind) -> u32 {
    use lang::index::SymbolKind::*;
    match k {
        Function => 3,
        Method => 2,
        Struct => 22,
        Enum => 13,
        Variant => 20,
        Field => 5,
        Module => 9,
        Trait => 8,
        Impl => 7,
        Const => 21,
        Local | Parameter => 6,
    }
}

/// A JSON array of already-rendered items.
fn list(items: impl Iterator<Item = String>) -> String {
    let mut out = String::from("[");
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&item);
    }
    out.push(']');
    out
}

/// What a name is, shown as the file writes it.
///
/// The `range` is the name under the cursor, which is what an editor
/// underlines while the hover is open.
fn hover(map: &lang::LineMap, word: lang::Span, label: &str) -> String {
    let mut text = String::new();
    json::quote(&format!("```trust\n{label}\n```"), &mut text);
    format!(
        "{{\"contents\":{{\"kind\":\"markdown\",\"value\":{text}}},\"range\":{}}}",
        range(map, word)
    )
}

/// An outline, nested as the file is.
fn symbols(map: &lang::LineMap, list: &[lang::index::Symbol]) -> String {
    let mut out = String::from("[");
    for (i, s) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let (mut name, mut detail) = (String::new(), String::new());
        json::quote(&s.name, &mut name);
        json::quote(&s.detail, &mut detail);
        out.push_str(&format!(
            "{{\"name\":{name},\"detail\":{detail},\"kind\":{},\
               \"range\":{},\"selectionRange\":{},\"children\":{}}}",
            kind(s.kind),
            range(map, s.span),
            // The protocol requires the selection to sit inside the range,
            // and an `impl` has no name, so it selects its own first token.
            range(map, s.name_span),
            symbols(map, &s.children),
        ));
    }
    out.push(']');
    out
}

/// The protocol's numbering for what kind of thing a symbol is.
fn kind(k: lang::index::SymbolKind) -> u32 {
    use lang::index::SymbolKind::*;
    match k {
        Function => 12,
        Method => 6,
        Struct => 23,
        Enum => 10,
        Variant => 22,
        Field => 8,
        // A trait is an interface, which is the closest thing the protocol's
        // list of kinds has to one.
        // The protocol has a Module kind; a Trait is closest to Interface.
        Module => 2,
        Trait => 11,
        Impl => 5,
        Const => 14,
        // An outline shows what a file declares, and neither of these is
        // declared at the top level — but the kinds are one enum, so they
        // answer with the protocol's `Variable` rather than nothing.
        Local | Parameter => 13,
    }
}

/// Tell the client a request could not be done, and why.
///
/// `-32803` is `RequestFailed`, which is what the protocol reserves for a
/// request that was understood and refused.
fn fail(id: Option<&Json>, message: &str) {
    let mut text = String::new();
    json::quote(message, &mut text);
    reply_raw(
        id,
        &format!("\"error\":{{\"code\":-32803,\"message\":{text}}}"),
    );
}

/// Send a reply to a request that had an id.
fn reply(id: Option<&Json>, result: &str) {
    reply_raw(id, &format!("\"result\":{result}"));
}

/// One reply, with whatever `result` or `error` member it carries.
fn reply_raw(id: Option<&Json>, body: &str) {
    let id = match id {
        Some(Json::Num(n)) => format!("{}", *n as i64),
        Some(Json::Str(s)) => {
            let mut out = String::new();
            json::quote(s, &mut out);
            out
        }
        _ => return,
    };
    send(&format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},{body}}}"));
}

/// One diagnostic, in the protocol's coordinates: 0-based lines, and columns
/// in UTF-16 code units.
struct Diagnostic<'a> {
    start: (u32, u32),
    end: (u32, u32),
    message: &'a str,
}

/// Place one error, by the span it carries.
///
/// A span with no line is one the compiler synthesized rather than read
/// (`Span::NONE`), and belongs at the top of the file: it is a fault in this
/// compiler, and hiding it under the first thing a program wrote would blame
/// the program.
fn at<'a>(map: &lang::LineMap, e: &'a lang::SyntaxError) -> Diagnostic<'a> {
    if e.span.line == 0 {
        return Diagnostic {
            start: (0, 0),
            end: (0, 0),
            message: &e.message,
        };
    }
    let lo = map.pos(e.span.lo);
    let hi = map.pos(e.span.hi);
    Diagnostic {
        start: (lo.line - 1, lo.utf16),
        end: (hi.line - 1, hi.utf16),
        message: &e.message,
    }
}

/// Send the diagnostics for one file, which replaces whatever was sent last.
fn publish(uri: &str, found: &[Diagnostic<'_>]) {
    let mut items = String::new();
    for (i, d) in found.iter().enumerate() {
        if i > 0 {
            items.push(',');
        }
        let mut text = String::new();
        json::quote(d.message, &mut text);
        let ((sl, sc), (el, ec)) = (d.start, d.end);
        items.push_str(&format!(
            "{{\"range\":{{\"start\":{{\"line\":{sl},\"character\":{sc}}},\
               \"end\":{{\"line\":{el},\"character\":{ec}}}}},\
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
    fn a_diagnostic_lands_on_the_expression_that_is_wrong() {
        let src = "fn main() -> t27 {\n    let x: t9 = 5;\n    x\n}\n";
        let errs = lang::compile(src).expect_err("an error");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("expected t27"), "{errs:?}");
        // The tail `x` is on the third line, four characters in, and one
        // character wide — not the whole line, which is what this server sent
        // before a span existed to send instead.
        let d = at(&lang::LineMap::new(src), &errs[0]);
        assert_eq!((d.start, d.end), ((2, 4), (2, 5)));
    }

    #[test]
    fn a_column_is_counted_in_utf16_because_the_protocol_is() {
        // Two astral characters before the error: four UTF-16 code units for
        // two characters, and an editor that was told two would put the
        // squiggle in the wrong place.
        let src = "// 🙂🙂\nfn main() -> t27 { nope() }\n";
        let errs = lang::compile(src).expect_err("an error");
        let map = lang::LineMap::new(src);
        assert_eq!(map.pos(3).utf16, 3);
        assert_eq!(map.pos(5).utf16, 7);
        let d = at(&map, &errs[0]);
        assert_eq!(d.start.0, 1, "the second line: {errs:?}");
    }
}
