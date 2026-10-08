//! The markup an inline SVG root ([`crate::tree::inline_svg`]) is parsed
//! from: its subtree written out as an SVG document.
//!
//! The serialisation is the input to `usvg`'s XML parser (`roxmltree`), so
//! everything here is decided by what that parser accepts and what `usvg`
//! reads:
//!
//! - The root declares `xmlns="http://www.w3.org/2000/svg"` and
//!   `xmlns:xlink="http://www.w3.org/1999/xlink"`. Nodes created through the
//!   element PAPI carry no namespace, so these two declarations are the whole
//!   of the namespace handling: every element is an SVG element and an
//!   `xlink:href` attribute resolves. Author-written `xmlns` and `xmlns:*`
//!   attributes are dropped on every element: on the root they would be
//!   duplicates `roxmltree` rejects, and elsewhere they would move an element
//!   out of the SVG namespace, which `setAttribute` never does in a browser.
//! - Attributes are written in the order the element holds them, values escaped (`&`, `<`, `>`,
//!   `"`, and tab, line feed and carriage return as character references so attribute-value
//!   normalisation keeps them).
//! - Text nodes are written escaped (`&`, `<`, `>`), so a `<style>` element's sheet reaches `usvg`
//!   as its text.
//! - A name that is not an XML name, an element name with a colon, or an attribute prefix other
//!   than `xlink` and `xml` (an unbound prefix) would fail the whole parse. The element PAPI can
//!   write any name, so such an attribute is skipped, and such an element is skipped together with
//!   its subtree.
//! - Characters XML 1.0 does not allow in a document (C0 controls other than tab, line feed and
//!   carriage return; U+FFFE, U+FFFF) are dropped.
//! - Shadow roots are not part of the document: the walk follows
//!   [`Node::child_ids`](crate::Node::child_ids) only.
//!
//! The root's own `style` attribute is written like any other attribute.
//! `usvg` does not apply it twice: its `convert_doc` converts the root's
//! children, never the root as a group, and reads the root's presentation
//! attributes only through inheritance.

use crate::tree::document::{Document, NodeId};

const ROOT_NAMESPACES: &str =
    r#" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;

/// Every element name `usvg` 0.48 knows (`usvg::parser::svgtree::EId`). An
/// element outside this set is dropped by `usvg` with its whole subtree, so
/// a mutation under one cannot change the picture.
const SVG_ELEMENT_NAMES: [&str; 53] = [
    "a",
    "circle",
    "clipPath",
    "defs",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "g",
    "image",
    "line",
    "linearGradient",
    "marker",
    "mask",
    "path",
    "pattern",
    "polygon",
    "polyline",
    "radialGradient",
    "rect",
    "stop",
    "style",
    "svg",
    "switch",
    "symbol",
    "text",
    "textPath",
    "tref",
    "tspan",
    "use",
];

/// Whether `name` is an element `usvg` reads.
pub(crate) fn is_svg_element_name(name: &str) -> bool {
    SVG_ELEMENT_NAMES.binary_search(&name).is_ok()
}

/// One step of the serialisation walk: an element still to open, or one to
/// close.
enum Step<'a> {
    Enter(NodeId),
    Close(&'a str),
}

/// `root`'s subtree as an SVG document.
pub(crate) fn serialize<T>(document: &Document<T>, root: NodeId) -> String {
    let mut out = String::new();
    // A stack rather than recursion: the element PAPI can nest elements
    // arbitrarily deep, and the depth of this walk must not be the thread's.
    let mut stack = vec![Step::Enter(root)];
    while let Some(step) = stack.pop() {
        let id = match step {
            Step::Close(name) => {
                out.push_str("</");
                out.push_str(name);
                out.push('>');
                continue;
            }
            Step::Enter(id) => id,
        };
        let Some(node) = document.get(id) else {
            continue;
        };
        if node.is_text_node() {
            escape_into(node.text().unwrap_or_default(), false, &mut out);
            continue;
        }
        let Some(name) = node.tag_name().filter(|_| node.is_element()) else {
            continue;
        };
        if !is_ncname(name) {
            continue;
        }
        out.push('<');
        out.push_str(name);
        if id == root {
            out.push_str(ROOT_NAMESPACES);
        }
        for (attribute, value) in node.attributes() {
            if !is_writable_attribute(attribute) {
                continue;
            }
            out.push(' ');
            out.push_str(attribute);
            out.push_str("=\"");
            escape_into(value, true, &mut out);
            out.push('"');
        }
        let children = node.child_ids();
        if children.is_empty() {
            out.push_str("/>");
            continue;
        }
        out.push('>');
        stack.push(Step::Close(name));
        stack.extend(children.iter().rev().map(|&child| Step::Enter(child)));
    }
    out
}

/// Whether an attribute can be written without failing the parse, and is
/// not a namespace declaration ([`ROOT_NAMESPACES`] is the only one).
fn is_writable_attribute(name: &str) -> bool {
    match name.split_once(':') {
        None => name != "xmlns" && is_ncname(name),
        Some((prefix, local)) => matches!(prefix, "xlink" | "xml") && is_ncname(local),
    }
}

/// Appends `text` with the characters markup gives meaning to escaped, and
/// the characters XML does not allow dropped.
fn escape_into(text: &str, attribute: bool, out: &mut String) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\t' | '\n' | '\r' => out.push(character),
            '\u{0}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}' => {}
            _ => out.push(character),
        }
    }
}

/// Whether `name` is an XML 1.0 (fifth edition) `Name` without a colon.
fn is_ncname(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(is_name_start) && characters.all(is_name_char)
}

/// XML 1.0 `NameStartChar`, without the colon.
fn is_name_start(character: char) -> bool {
    matches!(character,
        'A'..='Z'
        | '_'
        | 'a'..='z'
        | '\u{C0}'..='\u{D6}'
        | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}'
        | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0 `NameChar`, without the colon.
fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{SVG_ELEMENT_NAMES, is_svg_element_name, serialize};
    use crate::test_common::Doc;

    fn svg_root(doc: &mut Doc) -> crate::NodeId {
        let root = doc.root;
        let svg = doc.dom.create_element("svg", ());
        doc.dom.append_child(root, svg);
        svg
    }

    #[test]
    fn the_name_list_is_sorted_and_names_what_usvg_reads() {
        assert!(SVG_ELEMENT_NAMES.is_sorted());
        for name in ["svg", "path", "clipPath", "linearGradient", "style", "use"] {
            assert!(is_svg_element_name(name), "{name}");
        }
        for name in ["view", "raw-text", "clippath", "foreignObject", "title"] {
            assert!(!is_svg_element_name(name), "{name}");
        }
    }

    #[test]
    fn the_root_declares_both_namespaces_and_attributes_keep_their_order() {
        let mut doc = Doc::new();
        let svg = svg_root(&mut doc);
        doc.dom.set_attribute(svg, "viewBox", "0 0 24 24");
        doc.dom.set_attribute(svg, "width", "48");
        doc.dom.set_attribute(svg, "id", "icon");
        assert_eq!(
            serialize(&doc.dom, svg),
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 24 24" width="48" id="icon"/>"#
        );
    }

    #[test]
    fn values_and_text_are_escaped() {
        let mut doc = Doc::new();
        let svg = svg_root(&mut doc);
        doc.dom
            .set_attribute(svg, "data-x", "a&b<c>d\"e\tf\ng\rh\u{1}i");
        let style = doc.dom.create_element("style", ());
        doc.dom.append_child(svg, style);
        let sheet = doc
            .dom
            .create_text_node("g > rect { fill: \"a&b\"; }\u{0}", ());
        doc.dom.append_child(style, sheet);
        assert_eq!(
            serialize(&doc.dom, svg),
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#,
                r#" data-x="a&amp;b&lt;c&gt;d&quot;e&#9;f&#10;g&#13;hi">"#,
                r#"<style>g &gt; rect { fill: "a&amp;b"; }</style></svg>"#,
            )
        );
    }

    #[test]
    fn a_nested_svg_is_written_inside_its_root_with_no_declarations_of_its_own() {
        let mut doc = Doc::new();
        let svg = svg_root(&mut doc);
        let inner = doc.dom.create_element("svg", ());
        doc.dom.append_child(svg, inner);
        doc.dom.set_attribute(inner, "x", "4");
        let rect = doc.dom.create_element("rect", ());
        doc.dom.append_child(inner, rect);
        assert_eq!(
            serialize(&doc.dom, svg),
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">"#,
                r#"<svg x="4"><rect/></svg></svg>"#,
            )
        );
    }

    #[test]
    fn names_that_would_fail_the_parse_are_skipped() {
        let mut doc = Doc::new();
        let svg = svg_root(&mut doc);
        doc.dom
            .set_attribute(svg, "xmlns", "http://example.com/other");
        doc.dom
            .set_attribute(svg, "xmlns:xlink", "http://www.w3.org/1999/xlink");
        doc.dom.set_attribute(svg, "1bad", "x");
        doc.dom.set_attribute(svg, "foo:bar", "x");
        doc.dom.set_attribute(svg, "xml:space", "preserve");
        let bad = doc.dom.create_element("not valid", ());
        doc.dom.append_child(svg, bad);
        let hidden = doc.dom.create_element("rect", ());
        doc.dom.append_child(bad, hidden);
        let prefixed = doc.dom.create_element("svg:rect", ());
        doc.dom.append_child(svg, prefixed);
        let used = doc.dom.create_element("use", ());
        doc.dom.append_child(svg, used);
        doc.dom.set_attribute(used, "xlink:href", "#a");
        doc.dom
            .set_attribute(used, "xmlns", "http://example.com/other");
        assert_eq!(
            serialize(&doc.dom, svg),
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#,
                r##" xml:space="preserve"><use xlink:href="#a"/></svg>"##,
            )
        );
    }

    #[test]
    fn the_serialisation_parses() {
        let mut doc = Doc::new();
        let svg = svg_root(&mut doc);
        doc.dom.set_attribute(svg, "width", "10");
        doc.dom.set_attribute(svg, "height", "10");
        doc.dom
            .set_attribute(svg, "xmlns", "http://www.w3.org/2000/svg");
        let style = doc.dom.create_element("style", ());
        doc.dom.append_child(svg, style);
        let sheet = doc.dom.create_text_node("svg > rect { fill: red }", ());
        doc.dom.append_child(style, sheet);
        let rect = doc.dom.create_element("rect", ());
        doc.dom.append_child(svg, rect);
        doc.dom.set_attribute(rect, "width", "10");
        doc.dom.set_attribute(rect, "height", "10");
        doc.dom.set_attribute(rect, "bad name", "x");
        let markup = serialize(&doc.dom, svg);
        let image = crate::VectorImage::parse_sealed(markup.as_bytes()).expect("parses");
        assert_eq!(image.natural_size(), (10, 10));
    }
}
