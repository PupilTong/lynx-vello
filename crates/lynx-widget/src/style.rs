//! Lynx-specific adaptation of [`w3c_dom::StyleEngine`].
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
use stylo::servo_arc::Arc;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};
use w3c_dom::{Parallelism, StyleEngine as DomStyleEngine, StylesheetOrigin};

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
/// All standard CSS work and `SharedRwLock` ownership stay in the inner
/// [`w3c_dom::StyleEngine`]. This wrapper owns no second stylist or lock.
#[derive(Debug)]
pub struct StyleEngine {
    core: DomStyleEngine,
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
        let mut core = DomStyleEngine::new(build_device(metrics));
        core.add_stylesheet_str(&ua_stylesheet(page_config), StylesheetOrigin::UserAgent);
        Self { core, page_config }
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
    pub fn load_style_info(&mut self, info: &StyleInfo) {
        let rules = ingest::build_rules(&self.core, info);
        self.core.append_rules(rules, StylesheetOrigin::Author);
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
        self.core
            .flush_document_with(tree.document_mut(), parallelism);
    }

    /// Access the generic CSS engine.
    #[must_use]
    pub const fn core(&self) -> &DomStyleEngine {
        &self.core
    }

    /// Mutably access the generic CSS engine for standard device operations.
    pub const fn core_mut(&mut self) -> &mut DomStyleEngine {
        &mut self.core
    }

    /// Create a Widget tree bound to this engine's private style context.
    #[must_use]
    pub fn new_widget_tree(&self) -> WidgetTree {
        WidgetTree::from_document(self.core.new_document())
    }

    /// Parse and append a stylesheet that applies to all media.
    pub fn add_stylesheet_str(&mut self, css: &str, origin: StylesheetOrigin) {
        self.core.add_stylesheet_str(css, origin);
    }

    /// Parse and append a stylesheet with an explicit media query.
    pub fn add_stylesheet_with_media(
        &mut self,
        css: &str,
        origin: StylesheetOrigin,
        media_query: &str,
    ) {
        self.core
            .add_stylesheet_with_media(css, origin, media_query);
    }

    /// Resolve one Widget through the generic standards-oriented cascade.
    #[must_use]
    pub fn resolve_widget(
        &self,
        widget: WidgetRef<'_>,
        parent_style: Option<&ComputedValues>,
    ) -> Arc<ComputedValues> {
        self.core.resolve(widget, parent_style)
    }

    /// Update the Lynx view viewport.
    ///
    /// Already-styled trees keep computed values resolved against the old
    /// viewport (`rpx`/`vw`/`vh` lengths, media-dependent rules) until the
    /// embedder calls [`restyle_after_device_change`] on each of them.
    ///
    /// [`restyle_after_device_change`]: Self::restyle_after_device_change
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.core.set_viewport(width, height);
    }

    /// Update the device-pixel ratio while preserving the CSS viewport.
    ///
    /// As with [`set_viewport`](Self::set_viewport), follow up with
    /// [`restyle_after_device_change`](Self::restyle_after_device_change) on
    /// every styled tree.
    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.core.set_device_pixel_ratio(device_pixel_ratio);
    }

    /// Schedule a full restyle of `tree` after a device change
    /// ([`set_viewport`](Self::set_viewport) /
    /// [`set_device_pixel_ratio`](Self::set_device_pixel_ratio)): viewport
    /// units (`rpx`/`vw`/`vh`) re-resolve and media-dependent rules re-match
    /// on the tree's next flush. A no-op without a page root.
    pub fn restyle_after_device_change(&self, tree: &mut WidgetTree) {
        if let Some(page) = tree.page_id() {
            tree.document_mut().mark_subtree_dirty(page);
        }
    }
}

/// Build the touch-first servo device used by the Lynx adapter.
fn build_device(metrics: EngineMetrics) -> Device {
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
