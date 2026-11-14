//! The little JavaScript the runner understands.
//!
//! The engine has no JavaScript. A WPT script is accepted only when it is a
//! sequence of calls to known helpers (`test_valid_value`, `checkLayout`, …)
//! whose arguments are literals: strings, numbers, booleans, `null`, arrays
//! and objects of those. Anything else — a loop, a variable, a template
//! string with a substitution, a DOM mutation — makes the test one that
//! needs a script engine, and the runner reports it as such rather than
//! guessing what the script would have done.

/// A literal argument.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    Undefined,
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }
}

/// One top-level call: `name(args…)`.
#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub(crate) name: String,
    pub(crate) args: Vec<Value>,
}

/// Removes `//` and `/* */` comments, leaving string contents alone.
pub(crate) fn strip_comments(source: &str) -> String {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            out.push(c);
            i += 1;
            while i < bytes.len() {
                out.push(bytes[i]);
                if bytes[i] == '\\' && i + 1 < bytes.len() {
                    out.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '/' && bytes.get(i + 1) == Some(&'/') {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && bytes.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '$' || (c == '.' && self.pos > start) {
                self.pos += 1;
            } else {
                break;
            }
        }
        (self.pos > start).then(|| self.chars[start..self.pos].iter().collect())
    }

    fn string(&mut self, quote: char) -> Result<String, String> {
        let mut out = String::new();
        self.pos += 1;
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == quote {
                return Ok(out);
            }
            if quote == '`' && c == '$' && self.peek() == Some('{') {
                return Err("template substitution".into());
            }
            if c == '\\' {
                let escaped = self.peek().ok_or("unterminated escape")?;
                self.pos += 1;
                match escaped {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'u' => {
                        let hex: String = self.chars[self.pos..self.pos + 4].iter().collect();
                        self.pos += 4;
                        let code = u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                        out.push(char::from_u32(code).ok_or("bad escape")?);
                    }
                    '\n' => {}
                    other => out.push(other),
                }
                continue;
            }
            out.push(c);
        }
        Err("unterminated string".into())
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        let mut value = match self.peek().ok_or("unexpected end")? {
            q @ ('"' | '\'' | '`') => Value::Str(self.string(q)?),
            '[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    if self.eat(']') {
                        break;
                    }
                    items.push(self.value()?);
                    if !self.eat(',') {
                        if !self.eat(']') {
                            return Err("expected , or ]".into());
                        }
                        break;
                    }
                }
                Value::Array(items)
            }
            '{' => {
                self.pos += 1;
                let mut fields = Vec::new();
                loop {
                    if self.eat('}') {
                        break;
                    }
                    self.skip_ws();
                    let key = match self.peek() {
                        Some(q @ ('"' | '\'')) => self.string(q)?,
                        _ => self.ident().ok_or("expected key")?,
                    };
                    if !self.eat(':') {
                        return Err("expected :".into());
                    }
                    fields.push((key, self.value()?));
                    if !self.eat(',') {
                        if !self.eat('}') {
                            return Err("expected , or }".into());
                        }
                        break;
                    }
                }
                Value::Object(fields)
            }
            c if c.is_ascii_digit() || c == '-' || c == '.' => {
                let start = self.pos;
                self.pos += 1;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let text: String = self.chars[start..self.pos].iter().collect();
                Value::Num(text.parse().map_err(|_| format!("bad number {text}"))?)
            }
            _ => match self.ident().as_deref() {
                Some("true") => Value::Bool(true),
                Some("false") => Value::Bool(false),
                Some("null") => Value::Null,
                Some("undefined") => Value::Undefined,
                other => return Err(format!("non-literal argument {other:?}")),
            },
        };
        // String concatenation of literals: `'a' + 'b'`.
        loop {
            self.skip_ws();
            if self.peek() == Some('+') {
                self.pos += 1;
                let rhs = self.value()?;
                value = match (value, rhs) {
                    (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                    _ => return Err("non-string concatenation".into()),
                };
            } else {
                break;
            }
        }
        Ok(value)
    }
}

/// Parses a script that is nothing but helper calls with literal arguments.
pub(crate) fn parse_calls(source: &str) -> Result<Vec<Call>, String> {
    let source = strip_comments(source);
    let mut parser = Parser::new(&source);
    let mut calls = Vec::new();
    loop {
        parser.skip_ws();
        if parser.peek().is_none() {
            break;
        }
        if parser.eat(';') {
            continue;
        }
        // `'use strict';`
        if matches!(parser.peek(), Some('"' | '\'')) {
            parser.value()?;
            continue;
        }
        let name = parser.ident().ok_or_else(|| {
            let rest: String = parser.chars[parser.pos..].iter().take(40).collect();
            format!("unsupported statement near `{rest}`")
        })?;
        if !parser.eat('(') {
            return Err(format!("`{name}` is not a call"));
        }
        let mut args = Vec::new();
        loop {
            if parser.eat(')') {
                break;
            }
            args.push(parser.value()?);
            if !parser.eat(',') {
                if !parser.eat(')') {
                    return Err(format!("unsupported argument list for `{name}`"));
                }
                break;
            }
        }
        calls.push(Call { name, args });
    }
    Ok(calls)
}

/// The selector lists a check-layout page passes to `checkLayout`, or why
/// the page's scripts do more than call it.
pub(crate) fn check_layout_selectors(scripts: &[String]) -> Result<Vec<String>, String> {
    let mut selectors = Vec::new();
    for script in scripts {
        let mut rest = strip_comments(script);
        // Calls first: `checkLayout('…')`, `checkLayout("…", true)`.
        while let Some(start) = rest.find("checkLayout(") {
            let after = &rest[start + "checkLayout(".len()..];
            let mut parser = Parser::new(after);
            let value = parser.value()?;
            let Value::Str(selector) = value else {
                return Err("checkLayout argument is not a string".into());
            };
            selectors.push(selector);
            let mut depth = 1;
            let mut end = 0;
            for (index, c) in after.char_indices().skip(parser.pos) {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = index + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = format!("{}{}", &rest[..start], &after[end..]);
        }
        // What may wrap the calls: load and font-readiness callbacks.
        let mut leftover = rest;
        for boilerplate in [
            "document.fonts.ready.then",
            "window.addEventListener",
            "document.addEventListener",
            "window.onload",
            "\"load\"",
            "'load'",
            "\"DOMContentLoaded\"",
            "'DOMContentLoaded'",
            "setup({explicit_done: true})",
            "setup({ explicit_done: true })",
            "function",
            "async",
            "await",
            "document.fonts.ready",
            "done()",
            "=>",
            "'use strict'",
            "\"use strict\"",
        ] {
            leftover = leftover.replace(boilerplate, "");
        }
        if leftover.chars().any(char::is_alphanumeric) {
            let snippet: String = leftover
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(60)
                .collect();
            return Err(format!("script does more than checkLayout: `{snippet}`"));
        }
    }
    Ok(selectors)
}
