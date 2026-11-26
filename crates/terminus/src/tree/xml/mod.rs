//! XML content in the form the tree builds nodes from: the analogue of a
//! browser's XML parser behind `DOMParser`. Parsed once, owned, `Send +
//! Sync`, so the bytes can be parsed on any thread and the nodes built on
//! the document's ([`build`]: `Document::append_xml`).
//!
//! The parse ([`with_xml`]) checks UTF-8 and parses with DTDs allowed; it
//! fails with an [`XmlError`]. Nothing bounds element nesting before
//! `roxmltree`, which recurses once per nested element: a document nested
//! deeper than the parsing thread's stack holds overflows it, which aborts
//! the process (`docs/svg-dom-design.md`, "Risk: element nesting is not
//! bounded before the XML parse"). One walk then keeps, at most
//! [`MAX_NESTING`] levels deep:
//!
//! - elements in a namespace the tree has elements for ([`XmlNamespace`]), local names with their
//!   case, each recording its namespace. An element in no namespace is read in the namespace the
//!   caller names (`unqualified`), or dropped when it names none; an element in any other namespace
//!   is dropped with its subtree. A dropped root fails the parse ([`XmlError::ForeignRoot`]);
//! - attributes in no namespace under their local name, attributes in the XML namespace as
//!   `xml:<local>` and in the `XLink` namespace as `xlink:<local>`. The tree's attributes have no
//!   namespace, so any other namespaced attribute is dropped;
//! - every text node, white space only included, wherever it is; `CDATA` is text. Comments and
//!   processing instructions make no node: the tree has no such node types.
//!
//! The tree has elements for the HTML and the SVG namespaces; XML content
//! builds SVG-namespace elements only, which is all its one caller, the SVG
//! layer (`crate::svg`), needs.
//!
//! The representation is two vectors and one string buffer: the nodes in
//! document order (preorder), each naming its subtree's end so a walk can
//! skip it, the attributes of every element back to back, and every name,
//! value and text in one `String` that the others index.
//!
//! Everything here is crate-private and nothing is on `terminus::link`: an
//! SVG subtree built with `append_xml` alone is not a complete one, because
//! `style` and `use` elements are applied by the SVG builder
//! (`Document::append_svg_markup`), not by the elements themselves.
//!
//! The files: `tree/xml/markup.rs` holds [`XmlMarkup`] and the walk,
//! [`build`] `Document::append_xml`, and this file the prologue every parse
//! starts with ([`with_xml`], [`MAX_NESTING`], [`XmlError`]).

pub(crate) mod build;
mod markup;

pub(crate) use markup::{XmlItem, XmlMarkup, XmlNamespace};

pub(crate) const MAX_NESTING: u32 = 256;

fn with_xml<R>(
    bytes: &[u8],
    read: impl FnOnce(&roxmltree::Document<'_>) -> Result<R, XmlError>,
) -> Result<R, XmlError> {
    let text = std::str::from_utf8(bytes).map_err(|_| XmlError::NotUtf8)?;
    let xml = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(XmlError::Xml)?;
    read(&xml)
}

#[derive(Debug)]
pub(crate) enum XmlError {
    NotUtf8,
    Xml(roxmltree::Error),
    ForeignRoot,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUtf8 => formatter.write_str("the document is not UTF-8"),
            Self::Xml(error) => write!(formatter, "the document is not well-formed XML: {error}"),
            Self::ForeignRoot => formatter
                .write_str("the root element is in a namespace the tree has no elements for"),
        }
    }
}

impl std::error::Error for XmlError {}
