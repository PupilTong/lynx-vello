//! Shared harness for the CSS behavior tests ported from the `LynxJS` C++
//! engine (`lynx/core/renderer/css/**_test.cc` / `**_unittest.cc`).

#![allow(dead_code)]

use euclid::{Scale, Size2D};
use selectors::matching::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
    matches_selector_list,
};
use stylo::color::AbsoluteColor;
use stylo::context::QuirksMode;
use stylo::device::Device;
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::style_structs::Font;
use stylo::properties::{ComputedValues, PropertyId, parse_style_attribute};
use stylo::queries::values::PrefersColorScheme;
use stylo::selector_parser::SelectorParser;
use stylo::servo::media_features::PointerCapabilities;
use stylo::servo_arc::Arc;
use stylo::stylesheets::{CssRuleType, UrlExtraData};
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};
use w3c_dom::{Document, FlushSummary, NodeId, StylesheetOrigin};

#[must_use]
pub fn url_data() -> UrlExtraData {
    UrlExtraData::from(url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

#[must_use]
pub fn device(width: f32, height: f32) -> Device {
    device_with(width, height, 1.0, PrefersColorScheme::Light)
}

#[must_use]
pub fn device_with(
    width: f32,
    height: f32,
    device_pixel_ratio: f32,
    scheme: PrefersColorScheme,
) -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        Size2D::<f32, CSSPixel>::new(width, height),
        Size2D::<f32, DevicePixel>::new(width * device_pixel_ratio, height * device_pixel_ratio),
        Scale::<f32, CSSPixel, DevicePixel>::new(device_pixel_ratio),
        Box::new(TestFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        scheme,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

#[derive(Debug)]
pub struct TestFontMetricsProvider;

impl FontMetricsProvider for TestFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}

/// A document + root, with node construction and mutation helpers.
/// Every mutation goes through `Document` methods, which carry their own
/// snapshot/invalidation bookkeeping.
#[derive(Debug)]
pub struct Doc {
    pub dom: Document<()>,
    pub root: NodeId,
}

impl Doc {
    #[must_use]
    pub fn new() -> Self {
        Self::with_device(device(800.0, 600.0))
    }

    #[must_use]
    pub fn with_css(css: &str) -> Self {
        let mut doc = Self::new();
        doc.add_css(css);
        doc
    }

    #[must_use]
    pub fn with_device(device: Device) -> Self {
        let mut dom = Document::new(device);
        let root = dom.create_element("page", ());
        dom.append_document_element(root);
        Self { dom, root }
    }

    pub fn add_css(&mut self, css: &str) {
        self.dom.add_stylesheet(css, StylesheetOrigin::Author);
    }

    pub fn add_ua_css(&mut self, css: &str) {
        self.dom.add_stylesheet(css, StylesheetOrigin::UserAgent);
    }

    pub fn el(&mut self, parent: NodeId, spec: &str) -> NodeId {
        let parsed = NodeSpec::parse(spec);
        let id = self.dom.create_element(&parsed.tag, ());
        if let Some(id_attribute) = parsed.id {
            self.dom.set_id_attribute(id, Some(&id_attribute));
        }
        for class in parsed.classes {
            self.dom.add_class(id, &class);
        }
        for (name, value) in parsed.attrs {
            self.dom.set_attribute(id, &name, &value);
        }
        self.dom.append_child(parent, id);
        id
    }

    pub fn els(&mut self, parent: NodeId, specs: &[&str]) -> Vec<NodeId> {
        specs.iter().map(|spec| self.el(parent, spec)).collect()
    }

    pub fn flush(&mut self) -> FlushSummary {
        self.dom.flush_styles()
    }

    #[must_use]
    pub fn style(&self, id: NodeId) -> Arc<ComputedValues> {
        self.dom
            .get(id)
            .expect("node id is live")
            .computed_style()
            .expect("doc.flush() must run before reading computed style")
    }

    #[must_use]
    pub fn value(&self, id: NodeId, longhand: &str) -> String {
        let property = PropertyId::parse_enabled_for_all_content(longhand)
            .unwrap_or_else(|()| panic!("unknown property `{longhand}`"));
        let declaration_id = property
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{longhand}` is a shorthand; assert its longhands"));
        self.style(id).computed_value_to_string(declaration_id)
    }

    #[must_use]
    pub fn color(&self, id: NodeId) -> AbsoluteColor {
        self.style(id).clone_color()
    }

    #[must_use]
    pub fn matches(&self, id: NodeId, selector: &str) -> bool {
        let list = SelectorParser::parse_author_origin_no_namespace(selector, &url_data())
            .unwrap_or_else(|_| panic!("selector `{selector}` must parse"));
        let node = self.dom.get(id).expect("node id is live");
        let node_handle = &node;
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        matches_selector_list(&list, node_handle, &mut context)
    }

    #[must_use]
    pub fn selector_parses(selector: &str) -> bool {
        SelectorParser::parse_author_origin_no_namespace(selector, &url_data()).is_ok()
    }

    pub fn add_class(&mut self, id: NodeId, class: &str) {
        self.dom.add_class(id, class);
    }

    pub fn remove_class(&mut self, id: NodeId, class: &str) {
        self.dom.remove_class(id, class);
    }

    pub fn set_id(&mut self, id: NodeId, value: Option<&str>) {
        self.dom.set_id_attribute(id, value);
    }

    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        self.dom.set_attribute(id, name, value);
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        self.dom.remove_attribute(id, name);
    }

    pub fn set_inline(&mut self, id: NodeId, css: &str) {
        self.dom.set_inline_style(id, css);
    }
}

impl Default for Doc {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed form of the [`Doc::el`] spec grammar.
struct NodeSpec {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
}

impl NodeSpec {
    fn parse(spec: &str) -> Self {
        let mut parsed = Self {
            tag: String::new(),
            id: None,
            classes: Vec::new(),
            attrs: Vec::new(),
        };
        let mut rest = spec;
        while !rest.is_empty() {
            let (kind, body_start) = match rest.as_bytes()[0] {
                b'#' => ('#', 1),
                b'.' => ('.', 1),
                b'[' => ('[', 1),
                _ => ('t', 0),
            };
            if kind == '[' {
                let end = rest.find(']').expect("unterminated `[` in node spec");
                let inner = &rest[1..end];
                let (name, value) = match inner.split_once('=') {
                    Some((name, value)) => (name, value.trim_matches(['"', '\''])),
                    None => (inner, ""),
                };
                parsed.attrs.push((name.to_owned(), value.to_owned()));
                rest = &rest[end + 1..];
                continue;
            }
            let body = &rest[body_start..];
            let end = body.find(['#', '.', '[']).unwrap_or(body.len());
            let token = &body[..end];
            assert!(!token.is_empty(), "empty token in node spec `{spec}`");
            match kind {
                't' => token.clone_into(&mut parsed.tag),
                '#' => parsed.id = Some(token.to_owned()),
                '.' => parsed.classes.push(token.to_owned()),
                _ => unreachable!(),
            }
            rest = &rest[body_start + end..];
        }
        if parsed.tag.is_empty() {
            "view".clone_into(&mut parsed.tag);
        }
        parsed
    }
}

#[must_use]
pub fn specified(property: &str, value: &str) -> Option<String> {
    let css = format!("{property}: {value}");
    let block = parse_style_attribute(
        &css,
        &url_data(),
        None,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    );
    if block.is_empty() {
        return None;
    }
    let id = PropertyId::parse_enabled_for_all_content(property).ok()?;
    let mut serialized = String::new();
    block.property_value_to_css(&id, &mut serialized).ok()?;
    (!serialized.is_empty()).then_some(serialized)
}

#[must_use]
pub fn parses(property: &str, value: &str) -> bool {
    specified(property, value).is_some()
}

#[must_use]
pub fn specificity(selector: &str) -> Option<(u32, u32, u32)> {
    let list = SelectorParser::parse_author_origin_no_namespace(selector, &url_data()).ok()?;
    let selector = list.slice().first()?;
    let packed = selector.specificity();
    Some((
        (packed >> 20) & 0x3FF,
        (packed >> 10) & 0x3FF,
        packed & 0x3FF,
    ))
}

#[must_use]
pub fn media_matches_on(
    query: &str,
    width: f32,
    height: f32,
    dpr: f32,
    scheme: PrefersColorScheme,
) -> bool {
    const PROBE: &str = ".probe { color: rgb(1, 2, 3) }";
    let mut doc: Document<()> = Document::new(device_with(width, height, dpr, scheme));
    doc.add_stylesheet_with_media(PROBE, StylesheetOrigin::Author, query);
    let probe = doc.create_element("view", ());
    doc.add_class(probe, "probe");
    let style = doc.resolve_style(doc.get(probe).expect("fresh node"), None);
    style.clone_color() == rgb(1, 2, 3)
}

#[must_use]
pub fn media_matches(query: &str) -> bool {
    media_matches_on(query, 800.0, 600.0, 1.0, PrefersColorScheme::Light)
}

#[must_use]
pub fn rgb(r: u8, g: u8, b: u8) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(r, g, b, 1.0)
}

#[must_use]
pub fn rgba(r: u8, g: u8, b: u8, alpha: f32) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(r, g, b, alpha)
}
