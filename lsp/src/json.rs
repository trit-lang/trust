//! Just enough JSON for the Language Server Protocol.
//!
//! A language server usually reaches for `serde_json`; this repository has no
//! external dependency at all, and a tool is a poor reason to acquire its
//! first. What is needed is small and bounded: read a request, find three or
//! four fields in it, and write a reply.
//!
//! It is a *reader* and a *writer*, not a data model — nothing here builds a
//! tree it then walks twice.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A parsed JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// Any number, kept as written: the protocol's ids and positions are
    /// integers, and nothing here does arithmetic on a fraction.
    Num(f64),
    /// A string, unescaped.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object. Ordered, so that a reply reads the same twice.
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// The value at a path of object keys, or `None` if the path is not there.
    ///
    /// `get(["params", "textDocument", "uri"])` is the shape every use has.
    pub fn get(&self, path: &[&str]) -> Option<&Json> {
        let mut at = self;
        for key in path {
            let Json::Obj(map) = at else { return None };
            at = map.get(*key)?;
        }
        Some(at)
    }

    /// The string at a path.
    pub fn str_at(&self, path: &[&str]) -> Option<&str> {
        match self.get(path)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The array at a path.
    pub fn arr_at(&self, path: &[&str]) -> Option<&[Json]> {
        match self.get(path)? {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
}

/// Parse a whole JSON document, or `None` if it is not one.
pub fn parse(text: &str) -> Option<Json> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let v = value(&chars, &mut at)?;
    Some(v)
}

fn skip(c: &[char], at: &mut usize) {
    while *at < c.len() && c[*at].is_whitespace() {
        *at += 1;
    }
}

fn value(c: &[char], at: &mut usize) -> Option<Json> {
    skip(c, at);
    match *c.get(*at)? {
        '{' => object(c, at),
        '[' => array(c, at),
        '"' => string(c, at).map(Json::Str),
        't' => word(c, at, "true").map(|()| Json::Bool(true)),
        'f' => word(c, at, "false").map(|()| Json::Bool(false)),
        'n' => word(c, at, "null").map(|()| Json::Null),
        _ => number(c, at),
    }
}

fn word(c: &[char], at: &mut usize, want: &str) -> Option<()> {
    for w in want.chars() {
        if *c.get(*at)? != w {
            return None;
        }
        *at += 1;
    }
    Some(())
}

fn object(c: &[char], at: &mut usize) -> Option<Json> {
    *at += 1; // `{`
    let mut map = BTreeMap::new();
    skip(c, at);
    if *c.get(*at)? == '}' {
        *at += 1;
        return Some(Json::Obj(map));
    }
    loop {
        skip(c, at);
        let key = string(c, at)?;
        skip(c, at);
        if *c.get(*at)? != ':' {
            return None;
        }
        *at += 1;
        map.insert(key, value(c, at)?);
        skip(c, at);
        match *c.get(*at)? {
            ',' => *at += 1,
            '}' => {
                *at += 1;
                return Some(Json::Obj(map));
            }
            _ => return None,
        }
    }
}

fn array(c: &[char], at: &mut usize) -> Option<Json> {
    *at += 1; // `[`
    let mut out = Vec::new();
    skip(c, at);
    if *c.get(*at)? == ']' {
        *at += 1;
        return Some(Json::Arr(out));
    }
    loop {
        out.push(value(c, at)?);
        skip(c, at);
        match *c.get(*at)? {
            ',' => *at += 1,
            ']' => {
                *at += 1;
                return Some(Json::Arr(out));
            }
            _ => return None,
        }
    }
}

fn string(c: &[char], at: &mut usize) -> Option<String> {
    if *c.get(*at)? != '"' {
        return None;
    }
    *at += 1;
    let mut out = String::new();
    loop {
        let ch = *c.get(*at)?;
        *at += 1;
        match ch {
            '"' => return Some(out),
            '\\' => {
                let e = *c.get(*at)?;
                *at += 1;
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'u' => {
                        // A UTF-16 escape, and a surrogate pair when it is
                        // one: an editor sends these for anything outside the
                        // basic plane, and this protocol carries source text.
                        let hi = hex4(c, at)?;
                        if (0xD800..0xDC00).contains(&hi) {
                            if *c.get(*at)? != '\\' || *c.get(*at + 1)? != 'u' {
                                return None;
                            }
                            *at += 2;
                            let lo = hex4(c, at)?;
                            let n = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                            char::from_u32(n)?
                        } else {
                            char::from_u32(hi)?
                        }
                    }
                    other => other,
                });
            }
            other => out.push(other),
        }
    }
}

fn hex4(c: &[char], at: &mut usize) -> Option<u32> {
    let mut n = 0;
    for _ in 0..4 {
        n = n * 16 + c.get(*at)?.to_digit(16)?;
        *at += 1;
    }
    Some(n)
}

fn number(c: &[char], at: &mut usize) -> Option<Json> {
    let start = *at;
    if *c.get(*at)? == '-' {
        *at += 1;
    }
    while c
        .get(*at)
        .is_some_and(|d| d.is_ascii_digit() || matches!(d, '.' | 'e' | 'E' | '+' | '-'))
    {
        *at += 1;
    }
    let text: String = c[start..*at].iter().collect();
    text.parse().ok().map(Json::Num)
}

/// Write a string as a JSON string, with the escapes the protocol requires.
pub fn quote(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has to be escaped; everything above
            // may be sent as itself, since the transport is UTF-8.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_read_back() {
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"initialize",
                      "params":{"textDocument":{"uri":"file:///a.tr"}}}"#;
        let v = parse(text).expect("parses");
        assert_eq!(v.str_at(&["method"]), Some("initialize"));
        assert_eq!(
            v.str_at(&["params", "textDocument", "uri"]),
            Some("file:///a.tr")
        );
        assert_eq!(v.get(&["id"]), Some(&Json::Num(1.0)));
        assert_eq!(v.str_at(&["params", "missing"]), None);
    }

    #[test]
    fn escapes_survive_both_ways() {
        // A surrogate pair, which is what an editor sends for anything
        // outside the basic plane — and this protocol carries source text,
        // which in this language may contain any of it.
        let v = parse(r#"{"t":"a\"b\\c\nd世🙂"}"#).expect("parses");
        let s = v.str_at(&["t"]).expect("a string");
        assert_eq!(s, "a\"b\\c\nd世🙂");
        let mut out = String::new();
        quote(s, &mut out);
        assert_eq!(
            parse(&format!("{{\"t\":{out}}}")).unwrap().str_at(&["t"]),
            Some(s)
        );
    }

    #[test]
    fn a_malformed_document_is_none_and_not_a_panic() {
        for bad in ["{", "", "{\"a\"}", "[1,", "\"unterminated", "{\"a\":}"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
