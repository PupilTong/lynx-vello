//! Minimal document harness for scene tests — a trimmed copy of
//! `dom`'s test harness (test modules cannot be imported across
//! crates).

#![allow(dead_code)]

use dom::{Device, Document, NodeId, StylesheetOrigin, standards_device};
use euclid::{Scale, Size2D};
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};

#[derive(Debug)]
struct TestFontMetricsProvider;

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

#[must_use]
pub(super) fn device(width: f32, height: f32) -> Device {
    standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(width, height),
        Size2D::<f32, DevicePixel>::new(width, height),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(TestFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

#[derive(Debug)]
pub(super) struct Doc {
    pub(super) dom: Document<()>,
    pub(super) root: NodeId,
}

impl Doc {
    #[must_use]
    pub(super) fn with_css(css: &str) -> Self {
        Self::with_css_sized(css, 800.0, 600.0)
    }

    /// [`Self::with_css`] at an explicit viewport. `capture_document` sizes the
    /// frame from the document's own device, so a screenshot case that wants a
    /// specific canvas has to say so here.
    #[must_use]
    pub(super) fn with_css_sized(css: &str, width: f32, height: f32) -> Self {
        let mut dom = Document::new(device(width, height));
        let root = dom.create_element("page", ());
        dom.append_document_element(root);
        dom.add_stylesheet(css, StylesheetOrigin::Author);
        Self { dom, root }
    }

    pub(super) fn el(&mut self, parent: NodeId, class: &str) -> NodeId {
        self.el_tag(parent, "view", class)
    }

    /// [`Self::el`] with an explicit tag, for cases where the tag is the point
    /// — a replaced `img`, say, whose UA rules the case's own CSS supplies.
    pub(super) fn el_tag(&mut self, parent: NodeId, tag: &str, class: &str) -> NodeId {
        let id = self.dom.create_element(tag, ());
        for name in class.split_whitespace() {
            self.dom.add_class(id, name);
        }
        self.dom.append_child(parent, id);
        id
    }

    pub(super) fn text(&mut self, parent: NodeId, text: &str) -> NodeId {
        let id = self.dom.create_text_node(text, ());
        self.dom.append_child(parent, id);
        id
    }
}
