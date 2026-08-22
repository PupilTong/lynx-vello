// Copyright 2026 The Lynx Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

// The workspace sets `unsafe_code = "warn"`, which any module can silence with a
// local `allow`. This crate holds no `unsafe` at all, and that is a property
// worth a machine check rather than a convention: `forbid` cannot be overridden
// from inside the crate, so introducing `unsafe` here has to be a deliberate
// edit to this line.
#![forbid(unsafe_code)]

//! Parser for the restricted single-file Lynx XML markup source format.
//!
//! Lynx XML is a source envelope carrying an engine-version requirement, one
//! required main-thread JavaScript program, one optional background-thread
//! program, and one optional stylesheet. It is not a general-purpose XML
//! dialect and does not describe an element tree. The main-thread program
//! creates that tree through the Lynx Element PAPI.
//!
//! This source format is not an encoding of either Lynx binary template
//! container. The crate neither decodes nor produces `.web.bundle` or
//! `.lynx.bundle` files.

use std::fmt;

const XML_DECLARATION_START: &str = "<?xml";
const XML_DECLARATION_END: &str = "?>";
const COMMENT_START: &str = "<!--";
const COMMENT_END: &str = "-->";
const CDATA_START: &str = "<![CDATA[";
const CDATA_END: &str = "]]>";
const LYNX_ROOT_CLOSING_TAG: &str = "</lynx>";
const BYTE_ORDER_MARK: char = '\u{feff}';

/// The metadata and source sections extracted from one valid Lynx XML document.
///
/// Missing optional sections are `None`; present but empty sections are
/// `Some("")`. The required main-thread section may itself be empty because
/// the source grammar treats presence and runtime executability separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LynxXml<'source> {
    /// The non-empty `<lynx engine-version="...">` attribute value.
    pub engine_version: &'source str,
    /// The optional `<style>` body.
    pub style: Option<&'source str>,
    /// The required `<script thread="main">` body.
    pub main_thread_script: &'source str,
    /// The optional `<script thread="background">` body.
    pub background_thread_script: Option<&'source str>,
}

/// Why a Lynx XML document could not be parsed.
///
/// [`ParseError::offset`] counts UTF-16 code units to match the public web
/// parser and its formatted errors. [`ParseError::byte_offset`] exposes the
/// corresponding UTF-8 byte boundary for Rust callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    offset: usize,
    byte_offset: usize,
    message: Box<str>,
}

impl ParseError {
    fn at(source: &str, byte_offset: usize, message: impl Into<Box<str>>) -> Self {
        debug_assert!(source.is_char_boundary(byte_offset));
        Self {
            offset: source[..byte_offset].encode_utf16().count(),
            byte_offset,
            message: message.into(),
        }
    }

    /// The UTF-16 code-unit offset at which parsing failed.
    ///
    /// This is the coordinate included by [`fmt::Display`] and matches
    /// JavaScript string indexing in the reference web parser.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// The UTF-8 byte offset at which parsing failed.
    #[must_use]
    pub const fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    /// The human-readable reason without the offset prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid TemplateBundle XML at offset {}: {}",
            self.offset, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Parses a complete Lynx XML source document.
///
/// Section bodies borrow directly from `source`. Malformed input returns the
/// first structural error and never yields a partial result.
///
/// # Example
///
/// ```
/// let source = r#"<lynx engine-version="4.2">
///   <style>.title { color: red; }</style>
///   <script thread="main"><![CDATA[globalThis.renderPage = () => {};]]></script>
/// </lynx>"#;
/// let parsed = lynx_xml::parse(source)?;
/// assert_eq!(parsed.engine_version, "4.2");
/// assert_eq!(parsed.style, Some(".title { color: red; }"));
/// assert!(parsed.main_thread_script.contains("renderPage"));
/// # Ok::<(), lynx_xml::ParseError>(())
/// ```
pub fn parse(source: &str) -> Result<LynxXml<'_>, ParseError> {
    Parser::new(source).parse()
}

#[derive(Clone, Copy)]
enum SectionSlot {
    Style,
    MainThread,
    BackgroundThread,
}

struct Parser<'source> {
    source: &'source str,
    position: usize,
    style: Option<&'source str>,
    main_thread_script: Option<&'source str>,
    background_thread_script: Option<&'source str>,
}

impl<'source> Parser<'source> {
    const fn new(source: &'source str) -> Self {
        Self {
            source,
            position: 0,
            style: None,
            main_thread_script: None,
            background_thread_script: None,
        }
    }

    fn parse(mut self) -> Result<LynxXml<'source>, ParseError> {
        if self.source.starts_with(BYTE_ORDER_MARK) {
            self.position = BYTE_ORDER_MARK.len_utf8();
        }
        self.consume_ignorable()?;

        if self.remaining().starts_with(XML_DECLARATION_START) {
            let declaration_start = self.position;
            let search_start = declaration_start + XML_DECLARATION_START.len();
            let Some(relative_end) = self.source[search_start..].find(XML_DECLARATION_END) else {
                return Err(ParseError::at(
                    self.source,
                    declaration_start,
                    "unterminated XML declaration",
                ));
            };
            self.position = search_start + relative_end + XML_DECLARATION_END.len();
            self.consume_ignorable()?;
        }

        if self.remaining().starts_with("<!") {
            self.consume_doctype()?;
            self.consume_ignorable()?;
        }

        if !is_opening_tag(self.source, self.position, "lynx") {
            return Err(ParseError::at(
                self.source,
                self.position,
                "expected '<lynx engine-version=\"...\">' root element",
            ));
        }
        let engine_version = self.consume_root_start()?;

        let root_closed = loop {
            self.consume_ignorable()?;
            if self.remaining().starts_with(LYNX_ROOT_CLOSING_TAG) {
                self.position += LYNX_ROOT_CLOSING_TAG.len();
                break true;
            }
            if self.position == self.source.len() {
                break false;
            }
            self.consume_section()?;
        };

        if !root_closed {
            return Err(ParseError::at(
                self.source,
                self.position,
                "missing closing tag '</lynx>'",
            ));
        }
        self.consume_ignorable()?;
        if self.position != self.source.len() {
            return Err(ParseError::at(
                self.source,
                self.position,
                "unexpected content after '</lynx>'",
            ));
        }
        let Some(main_thread_script) = self.main_thread_script else {
            return Err(ParseError::at(
                self.source,
                self.position,
                "missing '<script thread=\"main\">' section",
            ));
        };

        Ok(LynxXml {
            engine_version,
            style: self.style,
            main_thread_script,
            background_thread_script: self.background_thread_script,
        })
    }

    fn remaining(&self) -> &'source str {
        &self.source[self.position..]
    }

    fn consume_ignorable(&mut self) -> Result<(), ParseError> {
        loop {
            while self
                .source
                .as_bytes()
                .get(self.position)
                .is_some_and(|byte| is_ascii_whitespace(*byte))
            {
                self.position += 1;
            }
            if !self.remaining().starts_with(COMMENT_START) {
                return Ok(());
            }

            let comment_start = self.position;
            let search_start = comment_start + COMMENT_START.len();
            let Some(relative_end) = self.source[search_start..].find(COMMENT_END) else {
                return Err(ParseError::at(
                    self.source,
                    comment_start,
                    "unterminated comment",
                ));
            };
            self.position = search_start + relative_end + COMMENT_END.len();
        }
    }

    fn consume_doctype(&mut self) -> Result<(), ParseError> {
        let doctype_start = self.position;
        let search_start = doctype_start + 2;
        let Some(relative_end) = self.source[search_start..].find('>') else {
            return Err(ParseError::at(
                self.source,
                doctype_start,
                "unterminated doctype declaration",
            ));
        };
        let doctype_end = search_start + relative_end;
        let declaration = trim_ascii_whitespace(&self.source[search_start..doctype_end]);
        let Some(keyword_end) = find_first_ascii_whitespace(declaration) else {
            return Err(ParseError::at(
                self.source,
                doctype_start,
                "expected '<!doctype lynx>'",
            ));
        };
        if !declaration[..keyword_end].eq_ignore_ascii_case("doctype")
            || !trim_ascii_whitespace(&declaration[keyword_end..]).eq_ignore_ascii_case("lynx")
        {
            return Err(ParseError::at(
                self.source,
                doctype_start,
                "expected '<!doctype lynx>'",
            ));
        }

        self.position = doctype_end + 1;
        Ok(())
    }

    fn consume_root_start(&mut self) -> Result<&'source str, ParseError> {
        let root_start = self.position;
        let search_start = root_start + 1;
        let Some(relative_end) = self.source[search_start..].find('>') else {
            return Err(ParseError::at(
                self.source,
                root_start,
                "unterminated '<lynx>' opening tag",
            ));
        };
        let opening_tag_end = search_start + relative_end;
        let opening_tag = trim_ascii_whitespace(&self.source[search_start..opening_tag_end]);
        let attributes = find_first_ascii_whitespace(opening_tag).map_or("", |tag_name_end| {
            trim_ascii_whitespace(&opening_tag[tag_name_end..])
        });
        let Some(engine_version) = quoted_attribute_value(attributes, "engine-version") else {
            return Err(ParseError::at(
                self.source,
                root_start,
                "'<lynx>' requires exactly one non-empty 'engine-version' attribute",
            ));
        };
        if engine_version.is_empty() {
            return Err(ParseError::at(
                self.source,
                root_start,
                "'<lynx>' requires exactly one non-empty 'engine-version' attribute",
            ));
        }

        self.position = opening_tag_end + 1;
        Ok(engine_version)
    }

    fn consume_section(&mut self) -> Result<(), ParseError> {
        let section_start = self.position;
        if self.source.as_bytes()[section_start] != b'<' {
            return Err(ParseError::at(
                self.source,
                section_start,
                "unexpected content outside a section",
            ));
        }

        let search_start = section_start + 1;
        let Some(relative_end) = self.source[search_start..].find('>') else {
            return Err(ParseError::at(
                self.source,
                section_start,
                "unterminated opening tag",
            ));
        };
        let opening_tag_end = search_start + relative_end;
        let opening_tag = trim_ascii_whitespace(&self.source[search_start..opening_tag_end]);
        if opening_tag.is_empty() || opening_tag.starts_with('/') {
            return Err(ParseError::at(
                self.source,
                section_start,
                "unexpected closing tag",
            ));
        }

        let tag_name_end = find_first_ascii_whitespace(opening_tag);
        let (tag_name, attributes) = tag_name_end.map_or((opening_tag, ""), |end| {
            (
                &opening_tag[..end],
                trim_ascii_whitespace(&opening_tag[end..]),
            )
        });

        let (closing_tag, slot) = self.classify_section(section_start, tag_name, attributes)?;

        if self.slot(slot).is_some() {
            return Err(ParseError::at(
                self.source,
                section_start,
                format!("duplicate '<{opening_tag}>' section"),
            ));
        }

        let content_start = opening_tag_end + 1;
        let mut closing_tag_search_start = content_start;
        while self
            .source
            .as_bytes()
            .get(closing_tag_search_start)
            .is_some_and(|byte| is_ascii_whitespace(*byte))
        {
            closing_tag_search_start += 1;
        }
        if self.source[closing_tag_search_start..].starts_with(CDATA_START) {
            let cdata_start = closing_tag_search_start;
            let search_start = cdata_start + CDATA_START.len();
            let Some(relative_end) = self.source[search_start..].find(CDATA_END) else {
                return Err(ParseError::at(
                    self.source,
                    cdata_start,
                    "unterminated CDATA section",
                ));
            };
            closing_tag_search_start = search_start + relative_end + CDATA_END.len();
        }

        let Some(relative_closing_tag) = self.source[closing_tag_search_start..].find(closing_tag)
        else {
            return Err(ParseError::at(
                self.source,
                content_start,
                format!("missing closing tag '{closing_tag}'"),
            ));
        };
        let closing_tag_start = closing_tag_search_start + relative_closing_tag;
        let content = self.section_content(
            &self.source[content_start..closing_tag_start],
            content_start,
        )?;
        self.assign_slot(slot, content);
        self.position = closing_tag_start + closing_tag.len();
        Ok(())
    }

    fn classify_section(
        &self,
        section_start: usize,
        tag_name: &str,
        attributes: &str,
    ) -> Result<(&'static str, SectionSlot), ParseError> {
        match tag_name {
            "style" => {
                if !attributes.is_empty() {
                    return Err(ParseError::at(
                        self.source,
                        section_start,
                        "'<style>' does not accept attributes",
                    ));
                }
                Ok(("</style>", SectionSlot::Style))
            }
            "script" => match quoted_attribute_value(attributes, "thread") {
                Some("main") => Ok(("</script>", SectionSlot::MainThread)),
                Some("background") => Ok(("</script>", SectionSlot::BackgroundThread)),
                _ => Err(ParseError::at(
                    self.source,
                    section_start,
                    "'<script>' requires exactly one 'thread' attribute with value 'main' or 'background'",
                )),
            },
            _ => Err(ParseError::at(
                self.source,
                section_start,
                format!("unsupported top-level tag '<{tag_name}>'"),
            )),
        }
    }

    fn section_content(
        &self,
        content: &'source str,
        content_start: usize,
    ) -> Result<&'source str, ParseError> {
        let trimmed = trim_ascii_whitespace(content);
        if !trimmed.starts_with(CDATA_START) {
            return Ok(content);
        }
        if trimmed.len() < CDATA_START.len() + CDATA_END.len() || !trimmed.ends_with(CDATA_END) {
            return Err(ParseError::at(
                self.source,
                content_start,
                "unterminated CDATA section",
            ));
        }
        let cdata = &trimmed[CDATA_START.len()..trimmed.len() - CDATA_END.len()];
        if cdata.contains(CDATA_END) {
            return Err(ParseError::at(
                self.source,
                content_start,
                "unexpected content after the CDATA section",
            ));
        }
        Ok(cdata)
    }

    const fn slot(&self, slot: SectionSlot) -> Option<&'source str> {
        match slot {
            SectionSlot::Style => self.style,
            SectionSlot::MainThread => self.main_thread_script,
            SectionSlot::BackgroundThread => self.background_thread_script,
        }
    }

    fn assign_slot(&mut self, slot: SectionSlot, content: &'source str) {
        match slot {
            SectionSlot::Style => self.style = Some(content),
            SectionSlot::MainThread => self.main_thread_script = Some(content),
            SectionSlot::BackgroundThread => self.background_thread_script = Some(content),
        }
    }
}

const fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn trim_ascii_whitespace(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && is_ascii_whitespace(bytes[start]) {
        start += 1;
    }
    while end > start && is_ascii_whitespace(bytes[end - 1]) {
        end -= 1;
    }
    &value[start..end]
}

fn find_first_ascii_whitespace(value: &str) -> Option<usize> {
    value
        .as_bytes()
        .iter()
        .position(|byte| is_ascii_whitespace(*byte))
}

fn is_opening_tag(source: &str, position: usize, tag_name: &str) -> bool {
    if source.as_bytes().get(position) != Some(&b'<')
        || !source[position + 1..].starts_with(tag_name)
    {
        return false;
    }
    let boundary = position + tag_name.len() + 1;
    source
        .as_bytes()
        .get(boundary)
        .is_some_and(|byte| *byte == b'>' || is_ascii_whitespace(*byte))
}

fn quoted_attribute_value<'attributes>(
    attributes: &'attributes str,
    attribute_name: &str,
) -> Option<&'attributes str> {
    let rest = attributes.strip_prefix(attribute_name)?;
    let rest = trim_ascii_whitespace(rest);
    let rest = rest.strip_prefix('=')?;
    let rest = trim_ascii_whitespace(rest);
    let &quote = rest.as_bytes().first()?;
    if rest.len() < 2 || !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let relative_closing_quote = rest.as_bytes()[1..]
        .iter()
        .position(|byte| *byte == quote)?;
    let closing_quote = relative_closing_quote + 1;
    (closing_quote == rest.len() - 1).then_some(&rest[1..closing_quote])
}
