//! `lynx-style` — the CSS style engine of **lynx-vello**.
//!
//! This crate drives the vendored, Lynx-patched **stylo** fork's cascade over
//! the [`lynx-dom`](lynx_dom) element tree to produce per-element
//! [`ComputedValues`]. It owns the servo [`Device`](stylo::device::Device) (the
//! Lynx view size + `rpx`/`ppx`/`sp` metric bases), the [`Stylist`], and the
//! [`SharedRwLock`] that guards every parsed rule and inline block.
//!
//! # Milestone scope (M2)
//!
//! This is the minimal wiring: build a [`StyleEngine`] from
//! [`EngineMetrics`], add author stylesheets from raw CSS text, and resolve a
//! single element (with an optional parent style for inheritance) into an
//! `Arc<ComputedValues>`. `StyleInfo` ingestion, the UA sheet, the flush-time
//! BFS driver, and `@media`/`@font-face`/`@keyframes` handling arrive in later
//! milestones — but the seams for them are kept open (see
//! [`StyleEngine::add_stylesheet_str`]).

mod device;

use cssparser::{Parser, ParserInput};
pub use device::EngineMetrics;
use device::build_device;
use lynx_dom::{Document, ElemRef};
use selectors::matching::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
};
use stylo::computed_value_flags::ComputedValueFlags;
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::dom::TElement;
use stylo::media_queries::MediaList;
use stylo::parser::ParserContext;
pub use stylo::properties::ComputedValues as ComputedStyle;
use stylo::properties::cascade::{FirstLineReparenting, cascade};
use stylo::properties::style_structs::Font;
use stylo::properties::{AnimationDeclarations, ComputedValues};
use stylo::rule_cache::RuleCacheConditions;
use stylo::rule_tree::RuleCascadeFlags;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{SharedRwLock, StylesheetGuards};
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::{
    AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData,
};
use stylo::stylist::{RuleInclusion, Stylist};
use stylo::values::specified::position::PositionTryFallbacksTryTactic;
use stylo_traits::ParsingMode;

/// The placeholder base URL for parsing stylesheets and media queries.
///
/// `about:blank` is a constant, valid URL, so this never fails.
fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// Whether `name` is a real (non-custom) CSS property in the vendored stylo
/// build lynx-vello compiles.
///
/// Useful for classifying properties against the tracking buckets: e.g.
/// `-webkit-text-stroke*` is present in stylo's source but gated
/// `engine = "gecko"`, so this returns `false` for it in the servo build —
/// which is why Lynx's `text-stroke*` must be fork-added (bucket B).
#[must_use]
pub fn property_is_supported(name: &str) -> bool {
    matches!(
        stylo::properties::PropertyId::parse_unchecked(name, None),
        Ok(stylo::properties::PropertyId::NonCustom(_))
    )
}

/// The style engine: a [`Stylist`] over the Lynx-extended servo [`Device`], the
/// [`SharedRwLock`] guarding its rules, and the base [`UrlExtraData`].
///
/// stylo owns the rule tree internally (unlike the 0.13 line, where it was a
/// separate `RuleTree`), reached via [`Stylist::rule_tree`].
///
/// **One engine per process**: the Lynx unit bases (`rpx`/`ppx`/`sp`) are
/// process-global in the stylo fork — see [`EngineMetrics`] for the full
/// contract. Constructing a second engine with different metrics silently
/// changes Lynx-unit resolution for the first.
///
/// [`Device`]: stylo::device::Device
pub struct StyleEngine {
    stylist: Stylist,
    lock: SharedRwLock,
    url_data: UrlExtraData,
}

impl std::fmt::Debug for StyleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Stylist` isn't `Debug`; surface the parts that are.
        f.debug_struct("StyleEngine")
            .field("viewport", &self.stylist.device().viewport_size())
            .field(
                "screen_width",
                &stylo::values::specified::lynx_units::lynx_screen_width_px(),
            )
            .field(
                "font_scale",
                &stylo::values::specified::lynx_units::lynx_font_scale(),
            )
            .finish_non_exhaustive()
    }
}

impl StyleEngine {
    /// Build a style engine for a Lynx view described by `metrics`.
    #[must_use]
    pub fn new(metrics: EngineMetrics) -> Self {
        let device = build_device(metrics);
        Self {
            stylist: Stylist::new(device, QuirksMode::NoQuirks),
            lock: SharedRwLock::new(),
            url_data: about_blank_url_data(),
        }
    }

    /// The engine's [`SharedRwLock`]. A [`Document`] must share this lock (via
    /// [`StyleEngine::new_document`] or [`Document::with_lock`]) for its inline
    /// style blocks to be readable by the cascade.
    #[must_use]
    pub fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    /// The engine's base [`UrlExtraData`].
    #[must_use]
    pub fn url_data(&self) -> &UrlExtraData {
        &self.url_data
    }

    /// Create an empty [`Document`] that shares this engine's lock, so its
    /// inline styles parse and cascade against the same [`SharedRwLock`].
    #[must_use]
    pub fn new_document(&self) -> Document {
        Document::with_lock(self.lock.clone(), self.url_data.clone())
    }

    /// Parse `css` as an author (or UA/user) stylesheet and add it to the
    /// [`Stylist`], flushing the cascade so its rules take effect immediately.
    ///
    /// Stylesheets are built through the media-capable
    /// [`Stylesheet::from_str`] path with a real (empty → "all") [`MediaList`],
    /// **not** a hardcoded `MediaList::empty()`. Today's `.web.bundle` wire
    /// format carries no `@media`, but a future wire-format extension will; at
    /// that point ingestion feeds the media-query text into
    /// [`StyleEngine::parse_media_list`] and stylo's existing `@media`
    /// evaluation does the rest.
    pub fn add_stylesheet_str(&mut self, css: &str, origin: Origin) {
        self.add_stylesheet_with_media(css, origin, "");
    }

    /// Like [`add_stylesheet_str`](Self::add_stylesheet_str) but with an
    /// explicit media-query string (`""` matches all media). This is the seam
    /// through which conditional (`@media`) sheets will flow once the wire
    /// format exposes them.
    pub fn add_stylesheet_with_media(&mut self, css: &str, origin: Origin, media_query: &str) {
        let media = self.parse_media_list(media_query);
        let sheet = Stylesheet::from_str(
            css,
            self.url_data.clone(),
            origin,
            media,
            self.lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );
        let document_sheet = DocumentStyleSheet(Arc::new(sheet));

        let guard = self.lock.read();
        self.stylist.append_stylesheet(document_sheet, &guard);
        let guards = StylesheetGuards::same(&guard);
        self.stylist.flush(&guards);
    }

    /// Parse a media-query string into a stylo [`MediaList`].
    ///
    /// This is the single place a [`MediaList`] is constructed, keeping the
    /// stylesheet path media-capable: an empty string parses to a list that
    /// matches all media, and any real media query flows straight into stylo's
    /// evaluator.
    #[must_use]
    pub fn parse_media_list(
        &self,
        media_query: &str,
    ) -> Arc<stylo::shared_lock::Locked<MediaList>> {
        let mut input = ParserInput::new(media_query);
        let mut parser = Parser::new(&mut input);
        let mut context = ParserContext::new(
            Origin::Author,
            &self.url_data,
            Some(CssRuleType::Media),
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            std::borrow::Cow::default(),
            None,
            None,
            AttrTaint::default(),
        );
        let media = MediaList::parse(&mut context, &mut parser);
        Arc::new(self.lock.wrap(media))
    }

    /// Resolve one element's computed style.
    ///
    /// `parent_style` is the parent element's [`ComputedValues`], used for
    /// inheritance; pass `None` at the root (stylo's initial values stand in).
    /// Mirrors Paws' `compute_style_for_node`, adapted to the stylo 0.19
    /// [`cascade`] signature. The ancestor bloom filter is deliberately `None`
    /// (see the plan); it is added only if a large-CSS benchmark demands it.
    #[must_use]
    pub fn resolve_element(
        &self,
        element: ElemRef<'_>,
        parent_style: Option<&ComputedValues>,
    ) -> Arc<ComputedValues> {
        let guard = self.lock.read();
        let guards = StylesheetGuards::same(&guard);

        let default_parent;
        let effective_parent = if let Some(parent) = parent_style {
            parent
        } else {
            default_parent =
                ComputedValues::initial_values_with_font_override(Font::initial_values());
            &default_parent
        };

        let mut selector_caches = SelectorCaches::default();
        let mut matching_context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut selector_caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );

        let mut applicable = stylo::applicable_declarations::ApplicableDeclarationList::new();
        self.stylist.push_applicable_declarations(
            element,
            None,
            element.style_attribute(),
            None,
            AnimationDeclarations::default(),
            RuleInclusion::All,
            &mut applicable,
            &mut matching_context,
        );

        let rule_node = self
            .stylist
            .rule_tree()
            .insert_ordered_rules_with_important(
                applicable
                    .into_iter()
                    .map(|block| (block.source.clone(), block.cascade_priority)),
                &guards,
            );

        let mut rule_cache_conditions = RuleCacheConditions::default();
        let mut tree_counting_caches = stylo::context::TreeCountingCaches::default();
        cascade::<ElemRef<'_>>(
            &self.stylist,
            None,
            &rule_node,
            &guards,
            Some(effective_parent),
            Some(effective_parent),
            FirstLineReparenting::No,
            &PositionTryFallbacksTryTactic::default(),
            None,
            ComputedValueFlags::empty(),
            RuleCascadeFlags::empty(),
            None,
            &mut rule_cache_conditions,
            None,
            &mut tree_counting_caches,
        )
    }

    /// Update the viewport (the Lynx view) on the [`Device`].
    ///
    /// A full restyle is the flush driver's job (M6); for now this just mutates
    /// the device and re-flushes the stylist so media evaluation is current.
    ///
    /// [`Device`]: stylo::device::Device
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        let device = self.stylist.device_mut();
        let dpr = device.device_pixel_ratio().get();
        device.set_viewport_size(euclid::Size2D::new(width, height));
        // Keep the build_device invariant `device_size = viewport * dpr`.
        device.set_device_size(euclid::Size2D::new(width * dpr, height * dpr));
        self.refresh_device();
    }

    /// Update the Lynx metric bases (`ppx` DPR, `rpx` screen width, `sp` font
    /// scale).
    ///
    /// The Device keeps its own DPR (for standard CSS resolution behavior);
    /// the Lynx units read the process-global metrics in stylo's
    /// `values::specified::lynx_units` — they are plain length units, so the
    /// servo [`Device`] does not know about them. Both are updated here, then
    /// the stylist is refreshed so the next resolve sees the new bases.
    ///
    /// [`Device`]: stylo::device::Device
    pub fn set_screen_metrics(
        &mut self,
        device_pixel_ratio: f32,
        screen_width: f32,
        font_scale: f32,
    ) {
        let device = self.stylist.device_mut();
        device.set_device_pixel_ratio(euclid::Scale::new(device_pixel_ratio));
        // Keep the build_device invariant `device_size = viewport * dpr`.
        let viewport = device.viewport_size();
        device.set_device_size(euclid::Size2D::new(
            viewport.width * device_pixel_ratio,
            viewport.height * device_pixel_ratio,
        ));
        stylo::values::specified::lynx_units::set_lynx_unit_metrics(
            screen_width,
            device_pixel_ratio,
            font_scale,
        );
        self.refresh_device();
    }

    /// Re-evaluate the stylist against the mutated device.
    ///
    /// stylo's `Device` mutators explicitly *do not* touch the associated
    /// `Stylist`, so we mark the origins whose media evaluation could have
    /// changed dirty and re-flush. Recomputing existing `ComputedValues`
    /// (so `rpx`/`vw` actually follow) is the flush driver's job in M6.
    fn refresh_device(&mut self) {
        let guard = self.lock.read();
        let guards = StylesheetGuards::same(&guard);
        let changed = self
            .stylist
            .media_features_change_changed_style(&guards, self.stylist.device());
        if !changed.is_empty() {
            self.stylist.force_stylesheet_origins_dirty(changed);
            self.stylist.flush(&guards);
        }
    }
}
