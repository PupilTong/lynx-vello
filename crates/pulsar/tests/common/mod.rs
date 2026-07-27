//! Minimal document harness for scene tests — a trimmed copy of
//! `w3c-dom`'s test harness (test modules cannot be imported across
//! crates).

#![allow(dead_code)]

use euclid::{Scale, Size2D};
use stylo::context::QuirksMode;
use stylo::device::Device;
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
use w3c_dom::{Document, NodeId, StylesheetOrigin};

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
pub fn device(width: f32, height: f32) -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
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
pub struct Doc {
    pub dom: Document<()>,
    pub root: NodeId,
}

impl Doc {
    #[must_use]
    pub fn with_css(css: &str) -> Self {
        let mut dom = Document::new(device(800.0, 600.0));
        let root = dom.create_element("page", ());
        dom.append_document_element(root);
        dom.add_stylesheet(css, StylesheetOrigin::Author);
        Self { dom, root }
    }

    pub fn el(&mut self, parent: NodeId, class: &str) -> NodeId {
        let id = self.dom.create_element("view", ());
        for name in class.split_whitespace() {
            self.dom.add_class(id, name);
        }
        self.dom.append_child(parent, id);
        id
    }

    pub fn text(&mut self, parent: NodeId, text: &str) -> NodeId {
        let id = self.dom.create_text_node(text, ());
        self.dom.append_child(parent, id);
        id
    }
}
