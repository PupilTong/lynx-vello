//! Shared production-host fixture for box-layout benchmarks.

#![allow(
    dead_code,
    reason = "each benchmark target compiles this shared fixture with a different method subset"
)]

use euclid::{Scale, Size2D};
use neutron_star::geometry::Size;
use style_traits::{CSSPixel, DevicePixel};
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
use stylo::values::computed::{CSSPixelLength, Display, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use w3c_dom::layout::Layout;
use w3c_dom::{Document, Node, NodeId};

const TEXT_SAMPLES: &[&str] = &[
    "Settings",
    "The quick brown fox jumps over the lazy dog.",
    "Text layout shapes once and reflows under the inline constraint.",
    "Responsive interfaces mix short labels with longer paragraphs.",
];

pub(super) const AHEM: &[u8] = include_bytes!("../../tests/fixtures/Ahem.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LeafContent {
    Synthetic,
    Text,
}

impl LeafContent {
    pub(super) const fn is_text(self) -> bool {
        matches!(self, Self::Text)
    }
}

#[derive(Debug)]
struct BenchFontMetrics;

impl FontMetricsProvider for BenchFontMetrics {
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

fn device(viewport: Size<f32>) -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        Size2D::<f32, CSSPixel>::new(viewport.width, viewport.height),
        Size2D::<f32, DevicePixel>::new(viewport.width, viewport.height),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetrics),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

/// A styled w3c-dom document ready for a cold production layout pass.
#[derive(Debug)]
pub(super) struct LayoutFixture {
    document: Document<()>,
    root: NodeId,
    node_count: usize,
    expected_display: Display,
    text_fonts_registered: bool,
}

impl LayoutFixture {
    pub(super) fn new(viewport: Size<f32>, root_style: &str) -> Self {
        let expected_display = root_style
            .split(';')
            .find_map(|declaration| match declaration.trim() {
                "display:flex" => Some(Display::Flex),
                "display:grid" => Some(Display::Grid),
                "display:linear" => Some(Display::Linear),
                "display:relative" => Some(Display::LynxRelative),
                _ => None,
            })
            .expect("every box benchmark root declares its production display mode");
        let mut document = Document::new(device(viewport));
        let root = document.create_element("page", ());
        document.set_inline_style(root, root_style);
        document.append_document_element(root);
        Self {
            document,
            root,
            node_count: 1,
            expected_display,
            text_fonts_registered: false,
        }
    }

    pub(super) fn root(&self) -> NodeId {
        self.root
    }

    pub(super) fn container(&mut self, parent: NodeId, style: &str) -> NodeId {
        self.push(parent, style, None)
    }

    pub(super) fn leaf(
        &mut self,
        parent: NodeId,
        style: &str,
        intrinsic: Size<f32>,
        first_baseline: Option<f32>,
    ) -> NodeId {
        self.push(parent, style, Some((intrinsic, first_baseline)))
    }

    pub(super) fn leaf_with_content(
        &mut self,
        parent: NodeId,
        style: &str,
        intrinsic: Size<f32>,
        first_baseline: Option<f32>,
        content: LeafContent,
        sample_index: usize,
    ) -> NodeId {
        match content {
            LeafContent::Synthetic => self.leaf(parent, style, intrinsic, first_baseline),
            LeafContent::Text => {
                let font_size = 12 + sample_index % 4 * 2;
                let style = format!(
                    "display:flex; align-items:flex-start; font-family:Ahem; font-size:{font_size}px; {style}"
                );
                let node = self.container(parent, &style);
                self.text(node, TEXT_SAMPLES[sample_index % TEXT_SAMPLES.len()]);
                node
            }
        }
    }

    pub(super) fn text(&mut self, parent: NodeId, text: &str) -> NodeId {
        if !self.text_fonts_registered {
            assert_eq!(self.document.register_fonts(AHEM), 1);
            self.text_fonts_registered = true;
        }
        let node = self.document.create_text_node(text, ());
        self.document.append_child(parent, node);
        self.node_count += 1;
        node
    }

    fn push(
        &mut self,
        parent: NodeId,
        style: &str,
        leaf_metrics: Option<(Size<f32>, Option<f32>)>,
    ) -> NodeId {
        let node = self.document.create_element("view", ());
        self.document.set_inline_style(node, style);
        self.document.append_child(parent, node);
        if let Some((size, first_baseline)) = leaf_metrics {
            self.document
                .set_leaf_metrics_for_testing(node, size, first_baseline);
        }
        self.node_count += 1;
        node
    }

    pub(super) fn prepare(mut self) -> Self {
        self.document.flush_styles();
        let display = self
            .document
            .get(self.root)
            .and_then(Node::computed_style)
            .expect("the attached benchmark root is styled")
            .clone_display();
        assert_eq!(
            display, self.expected_display,
            "the benchmark root's display declaration must reach w3c-dom"
        );
        self
    }

    pub(super) fn node_count(&self) -> usize {
        self.node_count
    }

    pub(super) fn run(&mut self) -> Layout {
        self.document.layout();
        self.node_layout(self.root)
    }

    pub(super) fn node_layout(&self, node: NodeId) -> Layout {
        self.document
            .get(node)
            .expect("benchmark node remains live")
            .rounded_layout()
            .clone()
    }

    pub(super) fn invalidate(&mut self, node: NodeId) {
        self.document.invalidate_layout(node);
    }

    pub(super) fn invalidate_root(&mut self) {
        self.document.invalidate_layout(self.root);
    }
}
