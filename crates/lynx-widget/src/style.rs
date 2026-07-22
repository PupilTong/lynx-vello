//! Lynx-specific construction and adaptation of document-owned style engines.
//!
//! The generic crate owns CSS parsing, matching, cascade, and locking. This
//! module supplies only the platform policy that is actually Lynx-specific:
//! view metrics, touch-first pointer capabilities, viewport-relative `rpx`,
//! and Widget-oriented convenience methods.

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
use crate::{WidgetRef, WidgetTree, ingest};

/// The environment metrics for a Lynx widget style engine.
///
/// Lengths are CSS pixels except `device_pixel_ratio`, which is a scale. Every
/// value lives on the stylo [`Device`], so engines remain independent. The
/// Lynx `rpx` unit is viewport-relative (`1rpx = viewport_width / 750`); the
/// fork deliberately does not support `ppx` or `sp`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineMetrics {
    /// The Lynx view width (basis for `vw` and `rpx`).
    pub viewport_width: f32,
    /// The Lynx view height (basis for `vh`).
    pub viewport_height: f32,
    /// CSS-pixel to device-pixel scale.
    pub device_pixel_ratio: f32,
}

impl EngineMetrics {
    /// Metrics for a `width` × `height` Lynx view with no font scaling.
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
///
/// Each tree created by this adapter receives a fresh `w3c_dom::Document` and
/// therefore its own stylist, stylesheets, device, and lock. This adapter only
/// retains the Lynx construction policy shared by those independent trees.
#[derive(Debug)]
pub struct StyleEngine {
    metrics: EngineMetrics,
    page_config: PageConfig,
}

impl StyleEngine {
    /// Build a Widget style engine for the supplied Lynx metrics, with the
    /// default page configuration.
    #[must_use]
    pub fn new(metrics: EngineMetrics) -> Self {
        Self::with_page_config(metrics, PageConfig::default())
    }

    /// Build a Widget style engine with an explicit page configuration.
    ///
    /// The configuration is honored **as generated UA styles** — a UA-origin
    /// stylesheet installed here (see [`crate::ua`]), never as branches in
    /// the styling engine.
    #[must_use]
    pub fn with_page_config(metrics: EngineMetrics, page_config: PageConfig) -> Self {
        Self {
            metrics,
            page_config,
        }
    }

    /// The page configuration this engine's UA styles were generated for.
    #[must_use]
    pub const fn page_config(&self) -> PageConfig {
        self.page_config
    }

    /// Load a decoded `StyleInfo` section: lower every fragment into stylo
    /// rules by direct construction (import flattening + cssId `:where`
    /// scoping included; see the crate-private `ingest` module) and mount them
    /// as one author
    /// stylesheet.
    pub fn load_style_info(&self, tree: &mut WidgetTree, info: &StyleInfo) {
        let rules = ingest::build_rules(tree.document(), info);
        tree.document_mut()
            .append_rules(rules, StylesheetOrigin::Author);
    }

    /// Restyle everything scheduled since the last flush (stylo's traversal:
    /// parallel when the tree is wide enough, invalidation-set-driven,
    /// style-sharing enabled). Styles land on the widgets; read them with
    /// [`WidgetTree::computed`].
    ///
    /// The core flush summary is intentionally not exposed here. Its harvest
    /// has already consumed relayout-class damage into the document's layout
    /// caches, so discarding the summary cannot lose later layout work.
    ///
    /// A no-op without a page root.
    pub fn flush_widget_tree(&self, tree: &mut WidgetTree) {
        self.flush_widget_tree_with(tree, Parallelism::Auto);
    }

    /// [`flush_widget_tree`](Self::flush_widget_tree) with explicit traversal
    /// scheduling (benchmarks pin [`Parallelism::Sequential`]).
    pub fn flush_widget_tree_with(&self, tree: &mut WidgetTree, parallelism: Parallelism) {
        // Reclaim handle-dropped detached subtrees before styling: the flush
        // is the reliable once-per-frame boundary.
        tree.sweep_dropped();
        tree.document_mut().flush_styles_with(parallelism);
    }

    /// Create an independent Widget tree and install its private UA
    /// stylesheet.
    #[must_use]
    pub fn new_widget_tree(&self) -> WidgetTree {
        let mut document = Document::new(build_device(self.metrics));
        document.add_stylesheet_str(
            &ua_stylesheet(self.page_config),
            StylesheetOrigin::UserAgent,
        );
        WidgetTree::from_document(document)
    }

    /// Parse and append a stylesheet that applies to all media.
    pub fn add_stylesheet_str(&self, tree: &mut WidgetTree, css: &str, origin: StylesheetOrigin) {
        tree.document_mut().add_stylesheet_str(css, origin);
    }

    /// Parse and append a stylesheet with an explicit media query.
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

    /// The number of registered `@font-face` rules.
    #[must_use]
    pub fn font_face_count(&self, tree: &WidgetTree) -> usize {
        tree.document().font_face_count()
    }

    /// Whether a named keyframes animation is available to `widget`.
    #[must_use]
    pub fn has_keyframes_animation(
        &self,
        tree: &WidgetTree,
        name: &str,
        widget: WidgetRef<'_>,
    ) -> bool {
        tree.document().has_keyframes_animation(name, widget)
    }

    /// Update the Lynx view viewport.
    ///
    /// The tree's document schedules itself internally in the same operation.
    pub fn set_viewport(&self, tree: &mut WidgetTree, width: f32, height: f32) {
        tree.document_mut().set_viewport(width, height);
    }

    /// Update the device-pixel ratio while preserving the CSS viewport.
    ///
    /// The tree's document schedules itself internally in the same operation.
    pub fn set_device_pixel_ratio(&self, tree: &mut WidgetTree, device_pixel_ratio: f32) {
        tree.document_mut()
            .set_device_pixel_ratio(device_pixel_ratio);
    }
}

/// Build the touch-first servo device used by the Lynx adapter.
pub(crate) fn build_device(metrics: EngineMetrics) -> Device {
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
