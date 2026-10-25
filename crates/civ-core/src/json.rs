// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A reader for the json dialect Unciv's rulesets are actually written in.
//!
//! Two reasons this is here rather than a dependency. The files use libGDX's
//! relaxed syntax — `//` and `/* */` comments, trailing commas — which strict
//! parsers reject, so a dependency would need the same preprocessing bolted in
//! front of it anyway. And numbers are kept as their original text rather than
//! parsed to `f64`, so [`Fixed`] can read `0.1` as exactly one hundred
//! thousandths instead of the nearest double. Going through a float first would
//! quietly reintroduce the non-determinism the whole crate is arranged to avoid.

use crate::stats::Fixed;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Kept as written, so it can be read exactly.
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// Insertion-ordered. Ruleset objects are small and order aids diffing.
    Object(Vec<(String, Json)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for Error {}

impl Json {
    pub fn parse(src: &str) -> Result<Json, Error> {
        let mut p = Parser { b: src.as_bytes(), i: 0, line: 1, col: 1 };
        p.skip_bom();
        p.ws()?;
        let v = p.value()?;
        p.ws()?;
        if p.i < p.b.len() {
            return Err(p.err("trailing content after the top-level value"));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            // `"cost": 40.0` appears alongside `"cost": 40`; both mean 40.
            Json::Number(n) => n.parse().ok().or_else(|| Fixed::parse(n).map(|f| f.round())),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }

    /// Exact, without passing through a float.
    pub fn as_fixed(&self) -> Option<Fixed> {
        match self {
            Json::Number(n) => Fixed::parse(n),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn strings(&self) -> Vec<String> {
        self.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    pub fn ints(&self) -> Vec<i32> {
        self.as_array().map(|a| a.iter().filter_map(|v| v.as_i32()).collect()).unwrap_or_default()
    }

    /// `{"Gold": 2}` style tables, which the ruleset uses for specialist slots,
    /// great-person points and percentage bonuses.
    pub fn counter(&self) -> std::collections::BTreeMap<String, i32> {
        self.as_object()
            .map(|m| m.iter().filter_map(|(k, v)| v.as_i32().map(|n| (k.clone(), n))).collect())
            .unwrap_or_default()
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    line: usize,
    col: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, m: impl Into<String>) -> Error {
        Error { line: self.line, column: self.col, message: m.into() }
    }

    fn skip_bom(&mut self) {
        if self.b.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.i = 3;
        }
    }

    fn bump(&mut self) -> Option<u8> {
        let c = *self.b.get(self.i)?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Whitespace and comments.
    fn ws(&mut self) -> Result<(), Error> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.bump();
                }
                Some(b'/') => match self.b.get(self.i + 1) {
                    Some(b'/') => {
                        while !matches!(self.peek(), None | Some(b'\n')) {
                            self.bump();
                        }
                    }
                    Some(b'*') => {
                        self.bump();
                        self.bump();
                        loop {
                            match self.peek() {
                                None => return Err(self.err("unterminated block comment")),
                                Some(b'*') if self.b.get(self.i + 1) == Some(&b'/') => {
                                    self.bump();
                                    self.bump();
                                    break;
                                }
                                _ => {
                                    self.bump();
                                }
                            }
                        }
                    }
                    _ => return Ok(()),
                },
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self) -> Result<Json, Error> {
        self.ws()?;
        match self.peek().ok_or_else(|| self.err("unexpected end of input"))? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Json::String(self.string()?)),
            b't' | b'f' | b'n' => self.literal(),
            _ => self.number(),
        }
    }

    fn literal(&mut self) -> Result<Json, Error> {
        for (word, out) in
            [(&b"true"[..], Json::Bool(true)), (&b"false"[..], Json::Bool(false)), (&b"null"[..], Json::Null)]
        {
            if self.b[self.i..].starts_with(word) {
                for _ in 0..word.len() {
                    self.bump();
                }
                return Ok(out);
            }
        }
        Err(self.err("expected true, false or null"))
    }

    fn number(&mut self) -> Result<Json, Error> {
        let start = self.i;
        if matches!(self.peek(), Some(b'-') | Some(b'+')) {
            self.bump();
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'-' || c == b'+')
        {
            self.bump();
        }
        if start == self.i {
            return Err(self.err("expected a value"));
        }
        Ok(Json::Number(String::from_utf8_lossy(&self.b[start..self.i]).into_owned()))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump().ok_or_else(|| self.err("unterminated string"))? {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = self.bump().ok_or_else(|| self.err("unterminated escape"))?;
                    out.push(match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let d = self.bump().ok_or_else(|| self.err("short \\u escape"))?;
                                code = code * 16
                                    + (d as char).to_digit(16).ok_or_else(|| self.err("bad hex in \\u escape"))?;
                            }
                            char::from_u32(code).unwrap_or('\u{fffd}')
                        }
                        other => return Err(self.err(format!("unknown escape \\{}", other as char))),
                    });
                }
                c if c < 0x80 => out.push(c as char),
                c => {
                    // Multi-byte utf-8: copy the remaining continuation bytes.
                    let len = if c >= 0xF0 {
                        4
                    } else if c >= 0xE0 {
                        3
                    } else {
                        2
                    };
                    let start = self.i - 1;
                    for _ in 1..len {
                        self.bump();
                    }
                    out.push_str(&String::from_utf8_lossy(&self.b[start..self.i]));
                }
            }
        }
    }

    /// Shared by arrays and objects: consume a separator, tolerating a trailing
    /// comma before the closing bracket.
    fn sep(&mut self, close: u8) -> Result<bool, Error> {
        self.ws()?;
        match self.peek() {
            Some(c) if c == close => {
                self.bump();
                Ok(false)
            }
            Some(b',') => {
                self.bump();
                self.ws()?;
                if self.peek() == Some(close) {
                    self.bump();
                    return Ok(false);
                }
                Ok(true)
            }
            _ => Err(self.err(format!("expected ',' or '{}'", close as char))),
        }
    }

    fn array(&mut self) -> Result<Json, Error> {
        self.bump();
        let mut out = Vec::new();
        self.ws()?;
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Json::Array(out));
        }
        loop {
            out.push(self.value()?);
            if !self.sep(b']')? {
                return Ok(Json::Array(out));
            }
        }
    }

    fn object(&mut self) -> Result<Json, Error> {
        self.bump();
        let mut out = Vec::new();
        self.ws()?;
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Json::Object(out));
        }
        loop {
            self.ws()?;
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a quoted key"));
            }
            let k = self.string()?;
            self.ws()?;
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':' after the key"));
            }
            self.bump();
            out.push((k, self.value()?));
            if !self.sep(b'}')? {
                return Ok(Json::Object(out));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_libgdx_dialect() {
        let v = Json::parse(
            r#"{
                // a line comment
                "name": "Library",   /* and a block one */
                "cost": 80,
                "isWonder": false,
                "uniques": ["[+1 Science] from every specialist", ],
            }"#,
        )
        .unwrap();
        assert_eq!(v.get("name").unwrap().as_str(), Some("Library"));
        assert_eq!(v.get("cost").unwrap().as_i32(), Some(80));
        assert_eq!(v.get("isWonder").unwrap().as_bool(), Some(false));
        assert_eq!(v.get("uniques").unwrap().strings().len(), 1);
    }

    #[test]
    fn numbers_stay_exact() {
        let v = Json::parse(r#"{"m": 0.1, "n": 1.25, "i": 40.0}"#).unwrap();
        assert_eq!(v.get("m").unwrap().as_fixed(), Some(Fixed::permille(100)));
        assert_eq!(v.get("n").unwrap().as_fixed(), Some(Fixed::permille(1_250)));
        // an integer written with a decimal point still reads as an integer
        assert_eq!(v.get("i").unwrap().as_i32(), Some(40));
    }

    #[test]
    fn handles_escapes_and_unicode() {
        let v = Json::parse(r#"{"q": "a \"b\" c\nd", "u": "é", "raw": "café"}"#).unwrap();
        assert_eq!(v.get("q").unwrap().as_str(), Some("a \"b\" c\nd"));
        assert_eq!(v.get("u").unwrap().as_str(), Some("é"));
        assert_eq!(v.get("raw").unwrap().as_str(), Some("café"));
    }

    #[test]
    fn reports_where_it_failed() {
        let e = Json::parse("{\n  \"a\": 1\n  \"b\": 2\n}").unwrap_err();
        assert_eq!(e.line, 3, "{e}");
    }

    #[test]
    fn rejects_trailing_content() {
        assert!(Json::parse("{} garbage").is_err());
        assert!(Json::parse("/* unterminated").is_err());
    }

    #[test]
    fn empty_containers() {
        assert_eq!(Json::parse("[]").unwrap(), Json::Array(vec![]));
        assert_eq!(Json::parse("{}").unwrap(), Json::Object(vec![]));
    }
}
