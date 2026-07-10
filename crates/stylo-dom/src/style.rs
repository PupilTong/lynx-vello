//! Standards-oriented CSS parsing, selector matching, and cascade execution.
//!
//! [`StyleEngine`] is the style-system owner for an embedder: it owns stylo's
//! [`Stylist`], the single [`SharedRwLock`] protecting stylesheet and inline
//! declaration blocks, and the base URL used while parsing CSS. Embedders
//! provide a stylo [`Device`] and an [`ExternalState`]
//! payload; no Lynx-specific metrics, units, or widget vocabulary live here.

use cssparser::{Parser, ParserInput};
use selectors::matching::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
};
use stylo::computed_value_flags::ComputedValueFlags;
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::device::Device;
use stylo::dom::TElement;
use stylo::media_queries::MediaList;
use stylo::parser::ParserContext;
/// The computed style produced by [`StyleEngine::resolve`].
pub use stylo::properties::ComputedValues as ComputedStyle;
use stylo::properties::cascade::{FirstLineReparenting, cascade};
use stylo::properties::style_structs::Font;
use stylo::properties::{AnimationDeclarations, ComputedValues};
use stylo::rule_cache::RuleCacheConditions;
use stylo::rule_tree::RuleCascadeFlags;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{SharedRwLock, StylesheetGuards};
/// A stylesheet's cascade origin.
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::{
    AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData,
};
use stylo::stylist::{RuleInclusion, Stylist};
use stylo::values::specified::position::PositionTryFallbacksTryTactic;
use stylo_traits::ParsingMode;

use crate::{Arena, ElementId, ElementRef, ExternalState};

/// The placeholder base URL used by [`StyleEngine::new`].
fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// Whether `name` is a real, non-custom property in this stylo build.
#[must_use]
pub fn property_is_supported(name: &str) -> bool {
    matches!(
        stylo::properties::PropertyId::parse_unchecked(name, None),
        Ok(stylo::properties::PropertyId::NonCustom(_))
    )
}

/// A generic stylo style engine for [`Arena`] trees.
///
/// The engine owns the only style lock an attached tree needs. Create styled
/// arenas with [`new_arena`](Self::new_arena); callers never need to construct,
/// share, or synchronize a `SharedRwLock` themselves.
pub struct StyleEngine {
    stylist: Stylist,
    lock: SharedRwLock,
    url_data: UrlExtraData,
}

impl std::fmt::Debug for StyleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleEngine")
            .field("viewport", &self.stylist.device().viewport_size())
            .field(
                "device_pixel_ratio",
                &self.stylist.device().device_pixel_ratio(),
            )
            .finish_non_exhaustive()
    }
}

impl StyleEngine {
    /// Build an engine around an embedder-supplied stylo [`Device`].
    #[must_use]
    pub fn new(device: Device) -> Self {
        Self::with_url_data(device, about_blank_url_data())
    }

    /// Build an engine with an explicit base URL for CSS parsing.
    #[must_use]
    pub fn with_url_data(device: Device, url_data: UrlExtraData) -> Self {
        Self {
            stylist: Stylist::new(device, QuirksMode::NoQuirks),
            lock: SharedRwLock::new(),
            url_data,
        }
    }

    /// Create an empty arena sharing this engine's private style context.
    #[must_use]
    pub fn new_arena<T>(&self) -> Arena<T> {
        Arena::with_style_context(self.lock.clone(), self.url_data.clone())
    }

    /// Inspect the device used for viewport units and media evaluation.
    #[must_use]
    pub fn device(&self) -> &Device {
        self.stylist.device()
    }

    /// Mutate the device and refresh media-dependent cascade data.
    ///
    /// Keeping device mutation behind this method prevents embedders from
    /// changing viewport state without also notifying the [`Stylist`].
    pub fn update_device(&mut self, update: impl FnOnce(&mut Device)) {
        update(self.stylist.device_mut());
        self.refresh_device();
    }

    /// Update the CSS viewport while preserving the current device-pixel ratio.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.update_device(|device| {
            let dpr = device.device_pixel_ratio().get();
            device.set_viewport_size(euclid::Size2D::new(width, height));
            device.set_device_size(euclid::Size2D::new(width * dpr, height * dpr));
        });
    }

    /// Update the device-pixel ratio while preserving the CSS viewport.
    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.update_device(|device| {
            device.set_device_pixel_ratio(euclid::Scale::new(device_pixel_ratio));
            let viewport = device.viewport_size();
            device.set_device_size(euclid::Size2D::new(
                viewport.width * device_pixel_ratio,
                viewport.height * device_pixel_ratio,
            ));
        });
    }

    /// Parse and append a stylesheet that applies to all media.
    pub fn add_stylesheet_str(&mut self, css: &str, origin: Origin) {
        self.add_stylesheet_with_media(css, origin, "");
    }

    /// Parse and append a stylesheet with an explicit media query.
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

    /// Parse a media-query string using this engine's URL and lock context.
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

    /// Match and cascade one element into standard CSS computed values.
    ///
    /// `parent_style` supplies inherited values. At a document root, pass
    /// `None` to inherit from stylo's initial values.
    #[must_use]
    pub fn resolve<T: ExternalState>(
        &self,
        element: ElementRef<'_, T>,
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
        cascade::<ElementRef<'_, T>>(
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

    /// Re-evaluate stylesheets whose media matching changed with the device.
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

impl<T> Arena<T> {
    /// Store a resolved style and mark the element's own style work complete.
    ///
    /// Returns `false` when `id` is stale. Descendant dirtiness is left intact
    /// for the future tree flush driver to process.
    pub fn store_computed_style(&mut self, id: ElementId, style: Arc<ComputedValues>) -> bool {
        let Some(element) = self.get_mut(id) else {
            return false;
        };
        element.computed = Some(style);
        element.style_dirty = false;
        true
    }
}
