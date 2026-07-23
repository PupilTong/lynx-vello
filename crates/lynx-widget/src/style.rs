//! Lynx-specific construction and adaptation of document-owned style engines.

use euclid::{Scale, Size2D};
use lynx_template_decoder::StyleInfo;
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
use w3c_dom::{Document, Parallelism, StylesheetOrigin};

use crate::ua::{PageConfig, ua_stylesheet};
use crate::{Widget, WidgetTree, ingest};

/// The environment metrics for a Lynx widget style engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewMetrics {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub device_pixel_ratio: f32,
}

impl ViewMetrics {
    #[must_use]
    pub fn new(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self {
            viewport_width: width,
            viewport_height: height,
            device_pixel_ratio,
        }
    }
}

/// The Widget-facing style adapter.
#[derive(Debug)]
pub struct StyleEngine {
    metrics: ViewMetrics,
    page_config: PageConfig,
}

impl StyleEngine {
    #[must_use]
    pub fn new(metrics: ViewMetrics) -> Self {
        Self::with_page_config(metrics, PageConfig::default())
    }

    #[must_use]
    pub fn with_page_config(metrics: ViewMetrics, page_config: PageConfig) -> Self {
        Self {
            metrics,
            page_config,
        }
    }

    #[must_use]
    pub const fn page_config(&self) -> PageConfig {
        self.page_config
    }

    pub fn load_style_info(&self, tree: &mut WidgetTree, info: &StyleInfo) {
        let rules = ingest::build_rules(tree.document(), info);
        tree.document_mut()
            .append_rules(rules, StylesheetOrigin::Author);
    }

    pub fn flush_styles(&self, tree: &mut WidgetTree) {
        self.flush_styles_with_parallelism(tree, Parallelism::Auto);
    }

    pub fn flush_styles_with_parallelism(&self, tree: &mut WidgetTree, parallelism: Parallelism) {
        tree.reclaim_detached_subtrees();
        tree.document_mut()
            .flush_styles_with_parallelism(parallelism);
    }

    #[must_use]
    pub fn new_tree(&self) -> WidgetTree {
        let mut document = Document::new(build_device(self.metrics));
        document.add_stylesheet(
            &ua_stylesheet(self.page_config),
            StylesheetOrigin::UserAgent,
        );
        WidgetTree::from_document(document)
    }

    pub fn add_stylesheet(&self, tree: &mut WidgetTree, css: &str, origin: StylesheetOrigin) {
        tree.document_mut().add_stylesheet(css, origin);
    }

    pub fn add_stylesheet_with_media(
        &self,
        tree: &mut WidgetTree,
        css: &str,
        origin: StylesheetOrigin,
        media_query: &str,
    ) {
        tree.document_mut()
            .add_stylesheet_with_media(css, origin, media_query);
    }

    #[must_use]
    pub fn font_face_count(&self, tree: &WidgetTree) -> usize {
        tree.document().font_face_count()
    }

    #[must_use]
    pub fn has_keyframes_animation(&self, tree: &WidgetTree, name: &str, widget: &Widget) -> bool {
        tree.document().has_keyframes_animation(name, widget)
    }

    pub fn set_viewport(&self, tree: &mut WidgetTree, width: f32, height: f32) {
        tree.document_mut().set_viewport(width, height);
    }

    pub fn set_device_pixel_ratio(&self, tree: &mut WidgetTree, device_pixel_ratio: f32) {
        tree.document_mut()
            .set_device_pixel_ratio(device_pixel_ratio);
    }
}

pub(crate) fn build_device(metrics: ViewMetrics) -> Device {
    let default_values = ComputedValues::initial_values_with_font_override(Font::initial_values());
    let viewport = Size2D::<f32, CSSPixel>::new(metrics.viewport_width, metrics.viewport_height);
    let device_size = Size2D::<f32, DevicePixel>::new(
        metrics.viewport_width * metrics.device_pixel_ratio,
        metrics.viewport_height * metrics.device_pixel_ratio,
    );
    let device_pixel_ratio = Scale::<f32, CSSPixel, DevicePixel>::new(metrics.device_pixel_ratio);

    Device::new(
        MediaType::screen(),
        stylo::context::QuirksMode::NoQuirks,
        viewport,
        device_size,
        device_pixel_ratio,
        Box::new(LynxFontMetricsProvider),
        default_values,
        PrefersColorScheme::Light,
        PointerCapabilities::COARSE,
        PointerCapabilities::COARSE,
    )
}

/// Temporary font metrics until the future Parley-backed text engine lands.
#[derive(Debug)]
struct LynxFontMetricsProvider;

impl FontMetricsProvider for LynxFontMetricsProvider {
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
