//! Shared production-host fixture for box-layout benchmarks.
//!
//! Construction and the initial style flush happen in Divan's input factory;
//! measured calls enter through `StyleEngine::layout_document`, exactly like
//! an embedder. The benchmark suite therefore exercises neutron-star through
//! w3c-dom's real `&Node` handle, computed styles, per-node caches, positioned
//! pass, and rounding pass instead of maintaining a benchmark-only host.

#![allow(
    dead_code,
    reason = "each benchmark target compiles this shared fixture with a different method subset"
)]

use euclid::{Scale, Size2D};
use neutron_star::geometry::{Point, Size};
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
use w3c_dom::layout::{Layout, LeafMeasureInput, LeafMetrics, MeasureLeaf};
use w3c_dom::{Document, ExternalState, Node, NodeId, StyleEngine};

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

/// Embedder payload used by benchmark leaves.
#[derive(Debug, Clone, Copy, Default)]
struct BenchLeaf {
    intrinsic: Size<f32>,
    first_baseline: Option<f32>,
}

impl ExternalState for BenchLeaf {}

impl MeasureLeaf for BenchLeaf {
    fn measure_leaf(&self, _node: &Node<Self>, input: LeafMeasureInput) -> LeafMetrics {
        let size = Size::new(
            input.known_dimensions.width.unwrap_or(self.intrinsic.width),
            input
                .known_dimensions
                .height
                .unwrap_or(self.intrinsic.height),
        );
        LeafMetrics::new(size).with_first_baselines(Point::new(None, self.first_baseline))
    }
}

/// A styled w3c-dom document ready for a cold production layout pass.
#[derive(Debug)]
pub(super) struct LayoutFixture {
    engine: StyleEngine,
    document: Document<BenchLeaf>,
    root: NodeId,
    node_count: usize,
    expected_display: Display,
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
        let engine = StyleEngine::new(device(viewport));
        let mut document = engine.new_document();
        let root = document.create_element("page", BenchLeaf::default());
        document.set_inline_style(root, root_style);
        document.append_child(root);
        Self {
            engine,
            document,
            root,
            node_count: 1,
            expected_display,
        }
    }

    pub(super) fn root(&self) -> NodeId {
        self.root
    }

    pub(super) fn container(&mut self, parent: NodeId, style: &str) -> NodeId {
        self.push(parent, style, BenchLeaf::default())
    }

    pub(super) fn leaf(
        &mut self,
        parent: NodeId,
        style: &str,
        intrinsic: Size<f32>,
        first_baseline: Option<f32>,
    ) -> NodeId {
        self.push(
            parent,
            style,
            BenchLeaf {
                intrinsic,
                first_baseline,
            },
        )
    }

    fn push(&mut self, parent: NodeId, style: &str, payload: BenchLeaf) -> NodeId {
        let node = self.document.create_element("view", payload);
        self.document.set_inline_style(node, style);
        self.document.append(parent, node);
        self.node_count += 1;
        node
    }

    /// Resolve all CSS outside the timed region while leaving layout caches cold.
    pub(super) fn prepare(mut self) -> Self {
        self.engine.flush_document(&mut self.document);
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
        self.engine.layout_document(&mut self.document);
        self.node_layout(self.root)
    }

    pub(super) fn node_layout(&self, node: NodeId) -> Layout {
        self.document
            .get(node)
            .expect("benchmark node remains live")
            .layout()
            .clone()
    }

    pub(super) fn invalidate(&mut self, node: NodeId) {
        self.document.invalidate_layout(node);
    }

    pub(super) fn invalidate_root(&mut self) {
        self.document.invalidate_layout(self.root);
    }
}
