//! The DOM-mutation scripts dynamic tests run, executed on the document.
//!
//! Many reftests and check-layout pages lay the page out once, change a style
//! or an attribute, and check or screenshot the result — they test that a
//! change relays out correctly, which `terminus` does incrementally. Their
//! scripts are straight-line code over a handful of DOM calls, so the runner
//! executes that subset directly instead of reporting them as needing a
//! script engine:
//!
//! - element expressions: `document.body`, `document.documentElement`,
//!   `document.getElementById('…')`, `document.querySelector('…')`, and a `var`/`let`/`const` bound
//!   to one of those;
//! - a layout read (`offsetTop`, `offsetWidth`, `clientHeight`, `scrollWidth`,
//!   `getBoundingClientRect()`, …), which flushes style and layout as a browser's forced layout
//!   does;
//! - `el.style.prop = '…'`, `el.style.setProperty('…', '…')`, `el.style.removeProperty('…')`,
//!   `el.style.cssText = '…'`;
//! - `el.className = '…'`, `el.id = '…'`, `el.classList.add/remove/toggle`, `el.setAttribute`,
//!   `el.removeAttribute`, `el.remove()`;
//! - `checkLayout('…')`, which checks at that point;
//! - the wrappers they sit in: `window.onload = function () { … }`, `onload = () => { … }`,
//!   `addEventListener('load', …)`, `requestAnimationFrame(…)`, `document.fonts.ready.then(…)`, and
//!   the `takeScreenshot()`/`done()` calls, which do nothing here.
//!
//! Callbacks run in the order they appear, after the page has loaded.
//! Anything else is an error, and the test is reported as needing a script
//! engine.

use terminus::link::NodeId;

use super::check_layout::{self, Subtest};
use super::loader::Page;
use super::script::strip_comments;

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num,
    Punct(char),
    Arrow,
}

fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = strip_comments(source).chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            tokens.push(Token::Num);
        } else if matches!(c, '"' | '\'' | '`') {
            let quote = c;
            let mut text = String::new();
            i += 1;
            loop {
                let Some(&next) = chars.get(i) else {
                    return Err("unterminated string".into());
                };
                i += 1;
                if next == quote {
                    break;
                }
                if quote == '`' && next == '$' && chars.get(i) == Some(&'{') {
                    return Err("template substitution".into());
                }
                if next == '\\' {
                    if let Some(&escaped) = chars.get(i) {
                        text.push(match escaped {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                        i += 1;
                    }
                    continue;
                }
                text.push(next);
            }
            tokens.push(Token::Str(text));
        } else if c == '=' && chars.get(i + 1) == Some(&'>') {
            tokens.push(Token::Arrow);
            i += 2;
        } else {
            tokens.push(Token::Punct(c));
            i += 1;
        }
    }
    Ok(tokens)
}

/// One effect of the script, in execution order.
#[derive(Clone, Debug)]
enum Op {
    Flush,
    Bind(String, Elem),
    SetStyle(Elem, String, String),
    RemoveStyle(Elem, String),
    CssText(Elem, String),
    ClassName(Elem, String),
    Id(Elem, String),
    ClassList(Elem, &'static str, Vec<String>),
    SetAttribute(Elem, String, String),
    RemoveAttribute(Elem, String),
    Remove(Elem),
    CheckLayout(String),
}

#[derive(Clone, Debug)]
enum Elem {
    Body,
    Root,
    ById(String),
    Query(String),
    Var(String),
}

const METRICS: &[&str] = &[
    "offsetTop",
    "offsetLeft",
    "offsetWidth",
    "offsetHeight",
    "offsetParent",
    "clientTop",
    "clientLeft",
    "clientWidth",
    "clientHeight",
    "scrollWidth",
    "scrollHeight",
    "scrollTop",
    "scrollLeft",
    "getBoundingClientRect",
    "getClientRects",
];

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    ops: Vec<Op>,
    /// Callbacks, run after the top-level statements.
    deferred: Vec<Vec<Op>>,
    vars: Vec<String>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    fn is_punct(&self, c: char) -> bool {
        self.peek() == Some(&Token::Punct(c))
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if self.is_punct(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, c: char) -> Result<(), String> {
        if self.eat_punct(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}`, found {:?}", self.peek()))
        }
    }

    fn is_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(ident)) if ident == name)
    }

    fn eat_ident(&mut self, name: &str) -> bool {
        if self.is_ident(name) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.peek().cloned() {
            Some(Token::Ident(ident)) => {
                self.pos += 1;
                Ok(ident)
            }
            other => Err(format!("expected a name, found {other:?}")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        match self.peek().cloned() {
            Some(Token::Str(text)) => {
                self.pos += 1;
                // `'a' + 'b'`.
                let mut text = text;
                while self.is_punct('+') {
                    self.pos += 1;
                    text.push_str(&self.string()?);
                }
                Ok(text)
            }
            other => Err(format!("expected a string, found {other:?}")),
        }
    }

    fn block(&mut self) -> Result<(), String> {
        while self.peek().is_some() && !self.is_punct('}') {
            self.statement()?;
        }
        Ok(())
    }

    /// A callback: `function () { … }`, `() => { … }`, `() => expr`, or a
    /// bare function name the page also defines (not supported).
    fn callback(&mut self) -> Result<(), String> {
        self.eat_ident("async");
        let saved = std::mem::take(&mut self.ops);
        if self.eat_ident("function") {
            if matches!(self.peek(), Some(Token::Ident(_))) {
                self.pos += 1;
            }
            self.params()?;
            self.expect_punct('{')?;
            self.block()?;
            self.expect_punct('}')?;
        } else {
            self.params()?;
            if self.peek() != Some(&Token::Arrow) {
                return Err("expected a callback".into());
            }
            self.pos += 1;
            if self.eat_punct('{') {
                self.block()?;
                self.expect_punct('}')?;
            } else {
                self.expression()?;
            }
        }
        let body = std::mem::replace(&mut self.ops, saved);
        self.deferred.push(body);
        Ok(())
    }

    fn params(&mut self) -> Result<(), String> {
        if matches!(self.peek(), Some(Token::Ident(_))) && self.peek_at(1) == Some(&Token::Arrow) {
            self.pos += 1;
            return Ok(());
        }
        self.expect_punct('(')?;
        while !self.eat_punct(')') {
            self.ident()?;
            self.eat_punct(',');
        }
        Ok(())
    }

    fn statement(&mut self) -> Result<(), String> {
        if self.eat_punct(';') {
            return Ok(());
        }
        if self.eat_punct('{') {
            self.block()?;
            return self.expect_punct('}');
        }
        if self.is_ident("var") || self.is_ident("let") || self.is_ident("const") {
            self.pos += 1;
            let name = self.ident()?;
            self.expect_punct('=')?;
            let elem = self.elem()?;
            self.vars.push(name.clone());
            self.ops.push(Op::Bind(name, elem));
            self.eat_punct(';');
            return Ok(());
        }
        // `window.onload = …` / `onload = …` / `document.body.onload = …`.
        let start = self.pos;
        self.eat_ident("window");
        self.eat_punct('.');
        if self.eat_ident("onload") && self.eat_punct('=') {
            self.callback()?;
            self.eat_punct(';');
            return Ok(());
        }
        self.pos = start;
        // `window.addEventListener('load', cb)` and the document's.
        if (self.is_ident("window") || self.is_ident("document"))
            && self.peek_at(1) == Some(&Token::Punct('.'))
            && self.peek_at(2) == Some(&Token::Ident("addEventListener".into()))
        {
            self.pos += 3;
            self.expect_punct('(')?;
            let event = self.string()?;
            if event != "load" && event != "DOMContentLoaded" {
                return Err(format!("listens for `{event}`"));
            }
            self.expect_punct(',')?;
            self.callback()?;
            self.expect_punct(')')?;
            self.eat_punct(';');
            return Ok(());
        }
        if self.eat_ident("requestAnimationFrame") {
            self.expect_punct('(')?;
            self.callback()?;
            self.expect_punct(')')?;
            self.eat_punct(';');
            return Ok(());
        }
        // `document.fonts.ready.then(cb)`.
        if self.is_ident("document")
            && self.peek_at(2) == Some(&Token::Ident("fonts".into()))
            && self.peek_at(4) == Some(&Token::Ident("ready".into()))
        {
            self.pos += 5;
            if self.eat_punct('.') && self.eat_ident("then") {
                self.expect_punct('(')?;
                self.callback()?;
                self.expect_punct(')')?;
            }
            self.eat_punct(';');
            return Ok(());
        }
        if self.is_ident("await") {
            self.pos += 1;
        }
        self.expression()?;
        self.eat_punct(';');
        Ok(())
    }

    fn expression(&mut self) -> Result<(), String> {
        for ignored in ["takeScreenshot", "done", "takeScreenshotDelayed"] {
            if self.is_ident(ignored) && self.peek_at(1) == Some(&Token::Punct('(')) {
                self.pos += 2;
                while !self.eat_punct(')') {
                    if self.peek().is_none() {
                        return Err("unterminated call".into());
                    }
                    self.pos += 1;
                }
                return Ok(());
            }
        }
        if self.eat_ident("checkLayout") {
            self.expect_punct('(')?;
            let selector = self.string()?;
            while !self.eat_punct(')') {
                if self.peek().is_none() {
                    return Err("unterminated checkLayout".into());
                }
                self.pos += 1;
            }
            self.ops.push(Op::CheckLayout(selector));
            return Ok(());
        }
        let elem = self.elem()?;
        self.member(elem)
    }

    fn elem(&mut self) -> Result<Elem, String> {
        if self.eat_ident("document") {
            self.expect_punct('.')?;
            let member = self.ident()?;
            return match member.as_str() {
                "body" => Ok(Elem::Body),
                "documentElement" => Ok(Elem::Root),
                "getElementById" | "querySelector" => {
                    self.expect_punct('(')?;
                    let argument = self.string()?;
                    self.expect_punct(')')?;
                    Ok(if member == "getElementById" {
                        Elem::ById(argument)
                    } else {
                        Elem::Query(argument)
                    })
                }
                other => Err(format!("document.{other}")),
            };
        }
        let name = self.ident()?;
        if self.vars.contains(&name) {
            Ok(Elem::Var(name))
        } else {
            Err(format!("unknown name `{name}`"))
        }
    }

    fn member(&mut self, elem: Elem) -> Result<(), String> {
        self.expect_punct('.')?;
        let member = self.ident()?;
        if METRICS.contains(&member.as_str()) {
            if self.eat_punct('(') {
                self.expect_punct(')')?;
            }
            // `el.getBoundingClientRect().width`.
            while self.eat_punct('.') {
                self.ident()?;
            }
            self.ops.push(Op::Flush);
            return Ok(());
        }
        match member.as_str() {
            "style" => {
                self.expect_punct('.')?;
                let property = self.ident()?;
                match property.as_str() {
                    "setProperty" => {
                        self.expect_punct('(')?;
                        let name = self.string()?;
                        self.expect_punct(',')?;
                        let value = self.string()?;
                        if self.eat_punct(',') {
                            self.string()?;
                        }
                        self.expect_punct(')')?;
                        self.ops.push(Op::SetStyle(elem, name, value));
                    }
                    "removeProperty" => {
                        self.expect_punct('(')?;
                        let name = self.string()?;
                        self.expect_punct(')')?;
                        self.ops.push(Op::RemoveStyle(elem, name));
                    }
                    "cssText" => {
                        self.expect_punct('=')?;
                        let value = self.string()?;
                        self.ops.push(Op::CssText(elem, value));
                    }
                    _ => {
                        self.expect_punct('=')?;
                        let value = match self.peek().cloned() {
                            Some(Token::Str(_)) => self.string()?,
                            other => return Err(format!("style value {other:?}")),
                        };
                        self.ops
                            .push(Op::SetStyle(elem, hyphenate(&property), value));
                    }
                }
            }
            "className" => {
                self.expect_punct('=')?;
                let value = self.string()?;
                self.ops.push(Op::ClassName(elem, value));
            }
            "id" => {
                self.expect_punct('=')?;
                let value = self.string()?;
                self.ops.push(Op::Id(elem, value));
            }
            "classList" => {
                self.expect_punct('.')?;
                let action = match self.ident()?.as_str() {
                    "add" => "add",
                    "remove" => "remove",
                    "toggle" => "toggle",
                    other => return Err(format!("classList.{other}")),
                };
                self.expect_punct('(')?;
                let mut classes = vec![self.string()?];
                while self.eat_punct(',') {
                    classes.push(self.string()?);
                }
                self.expect_punct(')')?;
                self.ops.push(Op::ClassList(elem, action, classes));
            }
            "setAttribute" => {
                self.expect_punct('(')?;
                let name = self.string()?;
                self.expect_punct(',')?;
                let value = self.string()?;
                self.expect_punct(')')?;
                self.ops.push(Op::SetAttribute(elem, name, value));
            }
            "removeAttribute" => {
                self.expect_punct('(')?;
                let name = self.string()?;
                self.expect_punct(')')?;
                self.ops.push(Op::RemoveAttribute(elem, name));
            }
            "remove" => {
                self.expect_punct('(')?;
                self.expect_punct(')')?;
                self.ops.push(Op::Remove(elem));
            }
            other => return Err(format!("element member `{other}`")),
        }
        Ok(())
    }
}

fn hyphenate(property: &str) -> String {
    let mut out = String::new();
    for c in property.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.starts_with("webkit-") {
        out.insert(0, '-');
    }
    out
}

/// Parses `scripts` (and the body's `onload` attribute) into the effects
/// they have, in execution order.
fn compile(scripts: &[String], onload: Option<&str>) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    let mut deferred = Vec::new();
    let mut vars = Vec::new();
    let mut sources: Vec<(&str, bool)> = scripts.iter().map(|s| (s.as_str(), false)).collect();
    if let Some(onload) = onload {
        sources.push((onload, true));
    }
    for (source, is_onload) in sources {
        let mut parser = Parser {
            tokens: tokenize(source)?,
            pos: 0,
            ops: Vec::new(),
            deferred: Vec::new(),
            vars: std::mem::take(&mut vars),
        };
        parser.block()?;
        if parser.peek().is_some() {
            return Err(format!("unsupported statement near {:?}", parser.peek()));
        }
        vars = parser.vars;
        if is_onload {
            deferred.push(parser.ops);
        } else {
            ops.extend(parser.ops);
        }
        deferred.extend(parser.deferred);
    }
    ops.extend(deferred.into_iter().flatten());
    Ok(ops)
}

/// Whether the scripts are within the subset; the page is left untouched.
pub(crate) fn supported(scripts: &[String], onload: Option<&str>) -> Result<(), String> {
    compile(scripts, onload).map(|_| ())
}

/// Runs the scripts on the page, returning the subtests their
/// `checkLayout` calls produced.
pub(crate) fn run(
    page: &mut Page,
    scripts: &[String],
    onload: Option<&str>,
) -> Result<Vec<Subtest>, String> {
    let ops = compile(scripts, onload)?;
    let mut vars: Vec<(String, Option<NodeId>)> = Vec::new();
    let mut results = Vec::new();
    for op in ops {
        let resolve = |page: &Page, vars: &[(String, Option<NodeId>)], elem: &Elem| {
            let root = page.dom.document_element().id();
            match elem {
                Elem::Body => page.body,
                Elem::Root => Some(root),
                Elem::ById(id) => page
                    .dom
                    .query_selector(root, &format!("[id=\"{id}\"]"))
                    .ok()
                    .flatten(),
                Elem::Query(selector) => page.dom.query_selector(root, selector).ok().flatten(),
                Elem::Var(name) => vars.iter().rev().find(|(n, _)| n == name).and_then(|v| v.1),
            }
        };
        let target = |elem: &Elem, vars: &[(String, Option<NodeId>)], page: &Page| {
            resolve(page, vars, elem).ok_or_else(|| format!("no element for {elem:?}"))
        };
        match op {
            Op::Flush => page.dom.layout(),
            Op::Bind(name, elem) => {
                let id = resolve(page, &vars, &elem);
                vars.push((name, id));
            }
            Op::SetStyle(elem, property, value) => {
                let id = target(&elem, &vars, page)?;
                // An empty value removes the property (CSSOM `setProperty`).
                let value = if property == "display" {
                    super::loader::emulate_block(&format!("display: {value}"))
                        .trim_start_matches("display: ")
                        .to_owned()
                } else {
                    value
                };
                page.dom.set_inline_style_property(id, &property, &value);
            }
            Op::RemoveStyle(elem, property) => {
                let id = target(&elem, &vars, page)?;
                page.dom.set_inline_style_property(id, &property, "");
            }
            Op::CssText(elem, value) => {
                let id = target(&elem, &vars, page)?;
                page.dom
                    .set_inline_style(id, &super::loader::emulate_block(&value));
            }
            Op::ClassName(elem, value) => {
                let id = target(&elem, &vars, page)?;
                page.dom.set_classes(id, &value);
            }
            Op::Id(elem, value) => {
                let id = target(&elem, &vars, page)?;
                page.dom.set_id_attribute(id, Some(&value));
            }
            Op::ClassList(elem, action, classes) => {
                let id = target(&elem, &vars, page)?;
                for class in classes {
                    let has = page.dom.get(id).is_some_and(|node| node.has_class(&class));
                    match (action, has) {
                        ("add" | "toggle", false) => page.dom.add_class(id, &class),
                        ("remove" | "toggle", true) => page.dom.remove_class(id, &class),
                        _ => {}
                    }
                }
            }
            Op::SetAttribute(elem, name, value) => {
                let id = target(&elem, &vars, page)?;
                match name.as_str() {
                    "style" => page
                        .dom
                        .set_inline_style(id, &super::loader::emulate_block(&value)),
                    "class" => page.dom.set_classes(id, &value),
                    "id" => page.dom.set_id_attribute(id, Some(&value)),
                    _ => page.dom.set_attribute(id, &name, &value),
                }
            }
            Op::RemoveAttribute(elem, name) => {
                let id = target(&elem, &vars, page)?;
                match name.as_str() {
                    "style" => page.dom.set_inline_style(id, ""),
                    "class" => page.dom.set_classes(id, ""),
                    "id" => page.dom.set_id_attribute(id, None),
                    _ => page.dom.remove_attribute(id, &name),
                }
            }
            Op::Remove(elem) => {
                let id = target(&elem, &vars, page)?;
                page.dom.remove_element(id);
            }
            Op::CheckLayout(selector) => {
                results.extend(check_layout::run(page, &[selector]));
            }
        }
    }
    Ok(results)
}
