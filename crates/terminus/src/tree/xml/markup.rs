//! [`XmlMarkup`]: a parsed XML document as the tree builds nodes from it,
//! and the walk that makes it from `roxmltree`'s tree ([module docs of the
//! XML layer](super)).

use std::ops::Range;

use super::{MAX_NESTING, XmlError, with_xml};

const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XmlNamespace {
    Svg,
}

impl XmlNamespace {
    fn of(uri: Option<&str>, unqualified: Option<Self>) -> Option<Self> {
        match uri {
            None => unqualified,
            Some(crate::svg::SVG_NAMESPACE) => Some(Self::Svg),
            Some(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct XmlMarkup {
    nodes: Vec<MarkupNode>,
    attributes: Vec<(Span, Span)>,
    strings: String,
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<XmlMarkup>();
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Copy)]
struct MarkupNode {
    content: Span,
    end: u32,
    attributes_start: u32,
    attributes_len: u16,
    namespace: XmlNamespace,
}

const TEXT: u16 = u16::MAX;

#[derive(Debug, Clone, Copy)]
pub(crate) enum XmlItem<'a> {
    Element {
        namespace: XmlNamespace,
        local_name: &'a str,
        attributes: &'a [(Span, Span)],
    },
    Text(&'a str),
}

impl XmlMarkup {
    pub(crate) fn parse(bytes: &[u8], unqualified: Option<XmlNamespace>) -> Result<Self, XmlError> {
        with_xml(bytes, |xml| {
            let root = xml.root_element();
            let namespace = XmlNamespace::of(root.tag_name().namespace(), unqualified)
                .ok_or(XmlError::ForeignRoot)?;
            let mut markup = Self {
                nodes: Vec::new(),
                attributes: Vec::new(),
                strings: String::new(),
            };
            markup.element(root, namespace, 1, unqualified);
            Ok(markup)
        })
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn item(&self, index: usize) -> XmlItem<'_> {
        let node = self.nodes[index];
        if node.attributes_len == TEXT {
            return XmlItem::Text(self.text(node.content));
        }
        let start = node.attributes_start as usize;
        XmlItem::Element {
            namespace: node.namespace,
            local_name: self.text(node.content),
            attributes: &self.attributes[start..start + usize::from(node.attributes_len)],
        }
    }

    pub(crate) fn children(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.nodes[index].end as usize;
        let mut next = index + 1;
        std::iter::from_fn(move || {
            (next < end).then(|| {
                let child = next;
                next = self.nodes[child].end as usize;
                child
            })
        })
    }

    pub(crate) fn subtree(&self, index: usize) -> Range<usize> {
        index..self.nodes[index].end as usize
    }

    pub(crate) fn text(&self, span: Span) -> &str {
        &self.strings[span.start as usize..(span.start + span.len) as usize]
    }

    pub(crate) fn attribute(&self, attribute: (Span, Span)) -> (&str, &str) {
        (self.text(attribute.0), self.text(attribute.1))
    }

    fn intern(&mut self, prefix: &str, text: &str) -> Span {
        let start = self.strings.len();
        self.strings.push_str(prefix);
        self.strings.push_str(text);
        Span {
            start: u32::try_from(start).expect("markup past 4 GiB"),
            len: u32::try_from(self.strings.len() - start).expect("markup past 4 GiB"),
        }
    }

    fn element(
        &mut self,
        node: roxmltree::Node<'_, '_>,
        namespace: XmlNamespace,
        depth: u32,
        unqualified: Option<XmlNamespace>,
    ) {
        let content = self.intern("", node.tag_name().name());
        let attributes_start = u32::try_from(self.attributes.len()).expect("markup past 4 GiB");
        for attribute in node.attributes() {
            let prefix = match attribute.namespace() {
                None => "",
                Some(XML_NAMESPACE) => "xml:",
                Some(XLINK_NAMESPACE) => "xlink:",
                Some(_) => continue,
            };
            let pair = (
                self.intern(prefix, attribute.name()),
                self.intern("", attribute.value()),
            );
            self.attributes.push(pair);
        }
        let attributes_len = u16::try_from(self.attributes.len() - attributes_start as usize)
            .ok()
            .filter(|len| *len != TEXT)
            .expect("roxmltree refuses an element with that many attributes");
        let index = self.nodes.len();
        self.nodes.push(MarkupNode {
            content,
            end: 0,
            attributes_start,
            attributes_len,
            namespace,
        });
        for child in node.children() {
            if child.is_element() {
                if depth < MAX_NESTING
                    && let Some(namespace) =
                        XmlNamespace::of(child.tag_name().namespace(), unqualified)
                {
                    self.element(child, namespace, depth + 1, unqualified);
                }
            } else if child.is_text()
                && let Some(text) = child.text()
            {
                let content = self.intern("", text);
                self.nodes.push(MarkupNode {
                    content,
                    end: u32::try_from(self.nodes.len() + 1).expect("markup past 4 GiB"),
                    attributes_start: 0,
                    attributes_len: TEXT,
                    namespace,
                });
            }
        }
        self.nodes[index].end = u32::try_from(self.nodes.len()).expect("markup past 4 GiB");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{MAX_NESTING, XmlError, XmlItem, XmlMarkup, XmlNamespace};

    const SVG: Option<XmlNamespace> = Some(XmlNamespace::Svg);

    fn svg(body: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:foo="urn:foo">{body}</svg>"#
        )
    }

    fn dump(markup: &XmlMarkup) -> Vec<String> {
        (0..markup.len())
            .map(|index| match markup.item(index) {
                XmlItem::Element {
                    local_name,
                    attributes,
                    ..
                } => {
                    let attributes: Vec<String> = attributes
                        .iter()
                        .map(|attribute| {
                            let (name, value) = markup.attribute(*attribute);
                            format!("{name}={value}")
                        })
                        .collect();
                    format!("{local_name}[{}]", attributes.join(","))
                }
                XmlItem::Text(text) => format!("{text:?}"),
            })
            .collect()
    }

    #[test]
    fn node_and_attribute_sizes() {
        assert_eq!(std::mem::size_of::<super::MarkupNode>(), 20);
        assert_eq!(std::mem::size_of::<(super::Span, super::Span)>(), 16);
    }

    #[test]
    fn parse_errors() {
        assert!(matches!(
            XmlMarkup::parse(b"\xff", SVG),
            Err(XmlError::NotUtf8)
        ));
        assert!(matches!(
            XmlMarkup::parse(b"<svg", SVG),
            Err(XmlError::Xml(_))
        ));
        assert!(
            matches!(
                XmlMarkup::parse(br#"<x:svg xmlns:x="urn:x"/>"#, SVG),
                Err(XmlError::ForeignRoot)
            ),
            "a root in a foreign namespace is dropped"
        );
    }

    #[test]
    fn keeps_elements_attributes_and_every_text() {
        let markup = XmlMarkup::parse(
            svg(r##"<linearGradient id="g"/><foo:bar><rect/></foo:bar>
                <use xlink:href="#g" x="1"/><use href="#a" xlink:href="#b"/><!-- c --><?p i?>
                <text xml:space="preserve" foo:x="1">a <tspan>b</tspan></text><![CDATA[d]]>"##)
            .as_bytes(),
            SVG,
        )
        .expect("parses");
        let gap = "\n                ";
        assert_eq!(
            dump(&markup),
            [
                "svg[]".to_owned(),
                "linearGradient[id=g]".to_owned(),
                format!("{gap:?}"),
                "use[xlink:href=#g,x=1]".to_owned(),
                "use[href=#a,xlink:href=#b]".to_owned(),
                format!("{gap:?}"),
                "text[xml:space=preserve]".to_owned(),
                "\"a \"".to_owned(),
                "tspan[]".to_owned(),
                "\"b\"".to_owned(),
                "\"d\"".to_owned(),
            ]
        );
        let root: Vec<usize> = markup.children(0).collect();
        assert_eq!(root, [1, 2, 3, 4, 5, 6, 10]);
        assert_eq!(markup.subtree(6), 6..10);
        assert!(matches!(
            markup.item(1),
            XmlItem::Element {
                namespace: XmlNamespace::Svg,
                ..
            }
        ));
    }

    #[test]
    fn white_space_text_is_kept() {
        let markup = XmlMarkup::parse(svg("\n  <g> </g>\n").as_bytes(), SVG).expect("parses");
        assert_eq!(
            dump(&markup),
            ["svg[]", "\"\\n  \"", "g[]", "\" \"", "\"\\n\""]
        );
    }

    #[test]
    fn no_namespace_follows_the_caller_and_a_foreign_one_is_dropped() {
        let bare = br#"<svg viewBox="0 0 4 4"><path d="M0 0h4"/></svg>"#;
        let kept = XmlMarkup::parse(bare, SVG).expect("markup without xmlns parses");
        assert_eq!(dump(&kept), ["svg[viewBox=0 0 4 4]", "path[d=M0 0h4]"]);
        assert!(matches!(
            XmlMarkup::parse(bare, None),
            Err(XmlError::ForeignRoot)
        ));
        let unqualified_child = XmlMarkup::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><g xmlns=""><path/></g><rect/></svg>"#,
            None,
        )
        .expect("parses");
        assert_eq!(dump(&unqualified_child), ["svg[]", "rect[]"]);
        let foreign = XmlMarkup::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><foo:bar xmlns:foo="urn:x"><path/>t</foo:bar></svg>"#,
            SVG,
        )
        .expect("parses");
        assert_eq!(dump(&foreign), ["svg[]"]);
    }

    #[test]
    fn depth_past_the_bound_is_cut() {
        let levels = MAX_NESTING as usize + 10;
        let markup = XmlMarkup::parse(
            svg(&("<g>".repeat(levels) + &"</g>".repeat(levels))).as_bytes(),
            SVG,
        )
        .expect("parses");
        assert_eq!(markup.len(), MAX_NESTING as usize, "the root is one deep");
    }
}
