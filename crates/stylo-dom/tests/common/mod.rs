//! Shared harness for the CSS behavior tests ported from the `LynxJS` C++
//! engine (`lynx/core/renderer/css/**_test.cc` / `**_unittest.cc`).
//!
//! Scope and expectation policy for every port (see
//! `docs/style-assumptions.md` and `docs/tracking/deviations.md`):
//!
//! - `enableCSSSelector = true` (NG selector path) and `enableRemoveCSSScope = true` (global
//!   styles) only.
//! - Real W3C features assert **W3C-correct** behavior — what stylo does — even where the C++
//!   engine deviates (e.g. selectors Lynx parses but never matches must match here; `var()` cycles
//!   use the spec's fallback rules).
//! - Lynx-only extensions (`display: linear`, `linear-*`/`relative-*` longhands, `rpx`/`ppx` units)
//!   assert Lynx's actual behavior.
//!
//! Tests build small trees through [`Doc`], flush through stylo's restyle
//! traversal, and assert computed values by serialized longhand
//! ([`Doc::value`]), typed color ([`Doc::color`]), raw selector matching
//! ([`Doc::matches`]), specified-value grammar round trips ([`specified`] /
//! [`parses`]), selector specificity ([`specificity`]), and media-query
//! evaluation ([`media_matches`]).

// Each integration-test crate compiles its own copy of this module and uses a
// subset of it.
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
use stylo_atoms::Atom;
use stylo_dom::{Arena, Element, ElementId, ElementState, StyleEngine, StylesheetOrigin};
use stylo_traits::{CSSPixel, DevicePixel};

/// The base URL every harness parse uses (mirrors `StyleEngine::new`).
#[must_use]
pub fn url_data() -> UrlExtraData {
    UrlExtraData::from(url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// A stylo `Device` for tests: `screen`, light scheme, no pointer.
#[must_use]
pub fn device(width: f32, height: f32) -> Device {
    device_with(width, height, 1.0, PrefersColorScheme::Light)
}

/// [`device`] with explicit device-pixel ratio and color scheme.
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

/// An engine + arena + root, with element construction and mutation helpers
/// whose snapshot ordering is always correct.
#[derive(Debug)]
pub struct Doc {
    pub engine: StyleEngine,
    pub arena: Arena<()>,
    pub root: ElementId,
}

impl Doc {
    /// An empty document (800×600 viewport) rooted at a `page` element.
    #[must_use]
    pub fn new() -> Self {
        Self::with_device(device(800.0, 600.0))
    }

    /// [`Doc::new`] plus one author stylesheet.
    #[must_use]
    pub fn with_css(css: &str) -> Self {
        let mut doc = Self::new();
        doc.add_css(css);
        doc
    }

    /// A document over an explicit device.
    #[must_use]
    pub fn with_device(device: Device) -> Self {
        let engine = StyleEngine::new(device);
        let mut arena = engine.new_arena();
        let root = arena.insert(Element::new("page", ()));
        Self {
            engine,
            arena,
            root,
        }
    }

    /// Append an author-origin stylesheet.
    pub fn add_css(&mut self, css: &str) {
        self.engine
            .add_stylesheet_str(css, StylesheetOrigin::Author);
    }

    /// Append a user-agent-origin stylesheet.
    pub fn add_ua_css(&mut self, css: &str) {
        self.engine
            .add_stylesheet_str(css, StylesheetOrigin::UserAgent);
    }

    /// Create an element from a spec string and attach it as `parent`'s last
    /// child.
    ///
    /// Spec grammar: `tag#id.class1.class2[attr=value][flag]` — tag first
    /// (defaults to `view` when the spec starts with `#`/`.`/`[`), then any
    /// number of `#id`, `.class`, and `[name]`/`[name=value]` parts. Attribute
    /// values may be single- or double-quoted.
    pub fn el(&mut self, parent: ElementId, spec: &str) -> ElementId {
        let parsed = ElementSpec::parse(spec);
        let mut element = Element::new(&parsed.tag, ());
        if let Some(id) = parsed.id {
            element.id_attr = Some(Atom::from(id.as_str()));
        }
        for class in parsed.classes {
            element.classes.push(Atom::from(class.as_str()));
        }
        for (name, value) in parsed.attrs {
            element.attrs.insert(name.into_boxed_str(), value);
        }
        let id = self.arena.insert(element);
        let index = self.arena.children_len(parent);
        self.arena.attach_at(parent, id, index);
        id
    }

    /// [`Doc::el`] for several children of one parent, in order.
    pub fn els(&mut self, parent: ElementId, specs: &[&str]) -> Vec<ElementId> {
        specs.iter().map(|spec| self.el(parent, spec)).collect()
    }

    /// Run a style flush (stylo's restyle traversal) over the whole tree.
    pub fn flush(&mut self) {
        self.engine.flush_tree(&mut self.arena, self.root);
    }

    /// The flushed computed style of `id`. Panics when `id` is stale or the
    /// tree has not been flushed since the element was styled.
    #[must_use]
    pub fn style(&self, id: ElementId) -> Arc<ComputedValues> {
        self.arena
            .get(id)
            .expect("element id is live")
            .computed_style()
            .expect("doc.flush() must run before reading computed style")
    }

    /// The computed value of one longhand, serialized to CSS text.
    #[must_use]
    pub fn value(&self, id: ElementId, longhand: &str) -> String {
        let property = PropertyId::parse_enabled_for_all_content(longhand)
            .unwrap_or_else(|()| panic!("unknown property `{longhand}`"));
        let declaration_id = property
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{longhand}` is a shorthand; assert its longhands"));
        self.style(id).computed_value_to_string(declaration_id)
    }

    /// The computed `color`.
    #[must_use]
    pub fn color(&self, id: ElementId) -> AbsoluteColor {
        self.style(id).clone_color()
    }

    /// Raw selector matching against the flushed-or-not tree (no rules
    /// involved). Panics when the selector list fails to parse.
    #[must_use]
    pub fn matches(&self, id: ElementId, selector: &str) -> bool {
        let list = SelectorParser::parse_author_origin_no_namespace(selector, &url_data())
            .unwrap_or_else(|_| panic!("selector `{selector}` must parse"));
        let element = self.arena.element_ref(id).expect("element id is live");
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        matches_selector_list(&list, &element, &mut context)
    }

    /// Whether `selector` parses at all in this build.
    #[must_use]
    pub fn selector_parses(selector: &str) -> bool {
        SelectorParser::parse_author_origin_no_namespace(selector, &url_data()).is_ok()
    }

    // --- Mutation helpers (snapshot before mutate, per dirty.rs contract) ---

    /// Add a class.
    pub fn add_class(&mut self, id: ElementId, class: &str) {
        self.arena.note_class_change(id);
        self.arena
            .get_mut(id)
            .expect("element id is live")
            .classes
            .push(Atom::from(class));
    }

    /// Remove a class (no-op when absent).
    pub fn remove_class(&mut self, id: ElementId, class: &str) {
        self.arena.note_class_change(id);
        let atom = Atom::from(class);
        self.arena
            .get_mut(id)
            .expect("element id is live")
            .classes
            .retain(|existing| *existing != atom);
    }

    /// Set or clear the id attribute.
    pub fn set_id(&mut self, id: ElementId, value: Option<&str>) {
        self.arena.note_id_change(id);
        self.arena.get_mut(id).expect("element id is live").id_attr = value.map(Atom::from);
    }

    /// Set an attribute value.
    pub fn set_attr(&mut self, id: ElementId, name: &str, value: &str) {
        self.arena.note_attribute_change(id, name);
        self.arena
            .get_mut(id)
            .expect("element id is live")
            .attrs
            .insert(name.into(), value.into());
    }

    /// Remove an attribute.
    pub fn remove_attr(&mut self, id: ElementId, name: &str) {
        self.arena.note_attribute_change(id, name);
        self.arena
            .get_mut(id)
            .expect("element id is live")
            .attrs
            .remove(name);
    }

    /// Set or clear dynamic pseudo-class state bits (`:hover`/`:active`/…).
    pub fn set_state(&mut self, id: ElementId, state: ElementState, on: bool) {
        self.arena.note_state_change(id);
        let element = self.arena.get_mut(id).expect("element id is live");
        if on {
            element.element_state.insert(state);
        } else {
            element.element_state.remove(state);
        }
    }

    /// Replace the element's inline `style` declarations.
    pub fn set_inline(&mut self, id: ElementId, css: &str) {
        self.arena.set_inline_styles(id, css);
    }
}

impl Default for Doc {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed form of the [`Doc::el`] spec grammar.
struct ElementSpec {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
}

impl ElementSpec {
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
                let end = rest.find(']').expect("unterminated `[` in element spec");
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
            assert!(!token.is_empty(), "empty token in element spec `{spec}`");
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

/// Parse `property: value` as a specified declaration and serialize it back
/// (shorthands re-serialize as shorthands). `None` = the grammar rejected it.
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

/// Whether the specified-value grammar accepts `property: value`.
#[must_use]
pub fn parses(property: &str, value: &str) -> bool {
    specified(property, value).is_some()
}

/// Selector specificity as the `(id, class, type)` triple.
///
/// The C++ tests pack these as `id*0x10000 + class*0x100 + type`; the
/// `selectors` crate packs 10-bit fields. Ports must compare **triples** (or
/// relative order), never raw packed integers.
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

/// Evaluate one media-query string end to end: does a rule guarded by it
/// apply on a `width`×`height`, `dpr`-scaled, `scheme` device?
#[must_use]
pub fn media_matches_on(
    query: &str,
    width: f32,
    height: f32,
    dpr: f32,
    scheme: PrefersColorScheme,
) -> bool {
    const PROBE: &str = ".probe { color: rgb(1, 2, 3) }";
    let mut engine = StyleEngine::new(device_with(width, height, dpr, scheme));
    engine.add_stylesheet_with_media(PROBE, StylesheetOrigin::Author, query);
    let mut arena = engine.new_arena();
    let probe = arena.insert(Element::new("view", ()));
    arena
        .get_mut(probe)
        .expect("fresh element")
        .classes
        .push(Atom::from("probe"));
    let style = engine.resolve(arena.element_ref(probe).expect("fresh element"), None);
    style.clone_color() == rgb(1, 2, 3)
}

/// [`media_matches_on`] with the default 800×600, 1× light-scheme device.
#[must_use]
pub fn media_matches(query: &str) -> bool {
    media_matches_on(query, 800.0, 600.0, 1.0, PrefersColorScheme::Light)
}

/// Opaque legacy sRGB color.
#[must_use]
pub fn rgb(r: u8, g: u8, b: u8) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(r, g, b, 1.0)
}

/// Legacy sRGB color with alpha.
#[must_use]
pub fn rgba(r: u8, g: u8, b: u8, alpha: f32) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(r, g, b, alpha)
}
