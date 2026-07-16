//! Standards-oriented CSS parsing, selector matching, and cascade execution.
//!
//! [`StyleEngine`] is the style-system owner for an embedder: it owns stylo's
//! [`Stylist`], the single [`SharedRwLock`] protecting stylesheet and inline
//! declaration blocks, and the base URL used while parsing CSS. Embedders
//! provide a stylo [`Device`] and an [`ExternalState`] payload; no
//! platform-specific metrics, units, or widget vocabulary live here.
//!
//! Styling runs **in place on the one tree**: create documents with
//! [`StyleEngine::new_document`] so they share this engine's private style
//! context, mutate them through [`Document`] methods, and
//! [`flush_document`](StyleEngine::flush_document) (see [`crate::flush`])
//! drives stylo's restyle traversal directly over the document's nodes — no
//! separate style tree is ever built.

use std::num::NonZeroU64;
use std::sync::atomic::AtomicBool;

use cssparser::{Parser, ParserInput, SourceLocation};
use selectors::matching::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
};
use stylo::computed_value_flags::ComputedValueFlags;
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::device::Device;
use stylo::dom::TElement;
use stylo::font_face::parse_font_face_block;
use stylo::media_queries::MediaList;
use stylo::parser::ParserContext;
/// The computed style produced by [`StyleEngine::resolve`].
pub use stylo::properties::ComputedValues as ComputedStyle;
use stylo::properties::cascade::{FirstLineReparenting, cascade};
use stylo::properties::declaration_block::parse_one_declaration_into;
use stylo::properties::style_structs::Font;
use stylo::properties::{
    AnimationDeclarations, ComputedValues, Importance, PropertyDeclarationBlock, PropertyId,
    SourcePropertyDeclaration,
};
use stylo::rule_cache::RuleCacheConditions;
use stylo::rule_tree::RuleCascadeFlags;
use stylo::selector_parser::SelectorParser;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{SharedRwLock, StylesheetGuards};
/// A single CSS rule, as built by the [`StyleEngine`] rule builders.
pub use stylo::stylesheets::CssRule;
/// A stylesheet's cascade origin.
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::keyframes_rule::{KeyframesRule, parse_keyframe_list};
use stylo::stylesheets::{
    AllowImportRules, CssRuleType, CssRules, DocumentStyleSheet, Origin, StyleRule, Stylesheet,
    StylesheetContents, UrlExtraData,
};
use stylo::stylist::{RuleInclusion, Stylist};
use stylo::values::KeyframesName;
use stylo::values::specified::position::PositionTryFallbacksTryTactic;
use stylo_traits::ParsingMode;

use crate::document::{about_blank_url_data, mint_identity};
use crate::{Document, ExternalState, Node, NodeId};

/// One declaration for direct rule construction: property name, value text,
/// and whether it carries `!important`.
pub type RawDeclaration<'a> = (&'a str, &'a str, bool);

/// Whether `name` is a real, non-custom property in this stylo build.
#[must_use]
pub fn property_is_supported(name: &str) -> bool {
    matches!(
        stylo::properties::PropertyId::parse_unchecked(name, None),
        Ok(stylo::properties::PropertyId::NonCustom(_))
    )
}

/// A generic stylo style engine for [`Document`] trees.
///
/// The engine owns the only style lock an attached document needs. Create
/// styled documents with [`new_document`](Self::new_document); callers never
/// need to construct, share, or synchronize a `SharedRwLock` themselves.
pub struct StyleEngine {
    stylist: Stylist,
    lock: SharedRwLock,
    url_data: UrlExtraData,
    /// Process-unique engine identity, stamped into every document created
    /// by [`new_document`](Self::new_document) and validated wherever this
    /// engine's stylist/lock meet a document (see
    /// [`assert_owns`](Self::assert_owns)).
    token: NonZeroU64,
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
            token: mint_identity(),
        }
    }

    /// Create an empty document sharing this engine's private style context.
    ///
    /// The document is permanently paired with this engine: flushing or
    /// resolving it through any other engine panics at the entry point (the
    /// crate-private `assert_owns` boundary check).
    #[must_use]
    pub fn new_document<T>(&self) -> Document<T> {
        Document::with_style_context(self.lock.clone(), self.url_data.clone(), Some(self.token))
    }

    /// Assert that `document` was created by **this** engine.
    ///
    /// Every entry point that runs this engine's stylist or takes this
    /// engine's lock against a document's nodes calls this first. Skipping it
    /// would not make a mismatched pair work — inline styles are locked under
    /// the creating engine's `SharedRwLock`, so stylo panics deep inside
    /// (`Locked::read_with called with a guard from an unrelated
    /// SharedRwLock`), and a document *without* inline styles would silently
    /// cascade against the wrong stylist — it only makes the failure late,
    /// obscure, or invisible. Failing here is the let-it-crash contract with
    /// a nameable cause.
    ///
    /// # Panics
    ///
    /// Panics when `document` was created by a different engine or standalone
    /// (`Document::new`).
    pub(crate) fn assert_owns<T>(&self, document: &Document<T>) {
        assert_eq!(
            document.core().engine_token,
            Some(self.token),
            "document is not paired with this StyleEngine (created by a              different engine or standalone)"
        );
    }

    /// The engine's stylist (crate-internal: the flush traversal needs it).
    pub(crate) fn stylist(&self) -> &Stylist {
        &self.stylist
    }

    /// The engine's shared style lock (crate-internal).
    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
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

    /// Append pre-built rules as one stylesheet of the given origin, without
    /// any CSS-text round trip, and flush the stylist.
    ///
    /// This is the mounting half of **direct construction** (see
    /// `docs/style-assumptions.md` §B.5): pair it with
    /// [`build_style_rule`](Self::build_style_rule) /
    /// [`build_keyframes_rule`](Self::build_keyframes_rule) /
    /// [`build_font_face_rule`](Self::build_font_face_rule).
    pub fn append_rules(&mut self, rules: Vec<CssRule>, origin: Origin) {
        let rules = CssRules::new(rules, &self.lock);
        let contents = StylesheetContents::from_rules(
            rules,
            origin,
            self.url_data.clone(),
            QuirksMode::NoQuirks,
        );
        let sheet = Stylesheet {
            contents: self.lock.wrap(contents),
            shared_lock: self.lock.clone(),
            media: Arc::new(self.lock.wrap(MediaList::empty())),
            disabled: AtomicBool::new(false),
        };
        let document_sheet = DocumentStyleSheet(Arc::new(sheet));

        let guard = self.lock.read();
        self.stylist.append_stylesheet(document_sheet, &guard);
        let guards = StylesheetGuards::same(&guard);
        self.stylist.flush(&guards);
    }

    /// Build a style rule from selector text plus individual declarations.
    ///
    /// One selector-list parse for the rule; one per-property value parse per
    /// declaration (shorthands expand normally, unknown/disabled properties
    /// and invalid values are dropped exactly like a text parse would).
    /// Returns `None` when the selector list fails to parse — the whole rule
    /// is invalid per CSS error handling.
    #[must_use]
    pub fn build_style_rule<'d>(
        &self,
        selectors: &str,
        declarations: impl IntoIterator<Item = RawDeclaration<'d>>,
    ) -> Option<CssRule> {
        let selectors =
            SelectorParser::parse_author_origin_no_namespace(selectors, &self.url_data).ok()?;
        let block = self.parse_declaration_block(declarations, CssRuleType::Style);
        Some(CssRule::Style(Arc::new(self.lock.wrap(StyleRule {
            selectors,
            block: Arc::new(self.lock.wrap(block)),
            rules: None,
            source_location: SourceLocation { line: 0, column: 0 },
        }))))
    }

    /// Build an `@keyframes` rule from its name and the keyframe-list body
    /// text (`"0% { … } 100% { … }"`).
    ///
    /// Keyframe stops are rare and tiny compared to style rules, so parsing
    /// the reassembled body through stylo's keyframe-list parser keeps this
    /// exact without a bespoke stop builder.
    #[must_use]
    pub fn build_keyframes_rule(&self, name: &str, body: &str) -> Option<CssRule> {
        if name.is_empty() {
            return None;
        }
        let mut context = self.parser_context(CssRuleType::Keyframes);
        let mut input = ParserInput::new(body);
        let mut parser = Parser::new(&mut input);
        let keyframes = parse_keyframe_list(&mut context, &mut parser, &self.lock);
        Some(CssRule::Keyframes(Arc::new(self.lock.wrap(
            KeyframesRule {
                name: KeyframesName::from_ident(name),
                keyframes,
                vendor_prefix: None,
                source_location: SourceLocation { line: 0, column: 0 },
            },
        ))))
    }

    /// Build an `@font-face` rule from its descriptor-block text
    /// (`"font-family: X; src: url(…);"`).
    #[must_use]
    pub fn build_font_face_rule(&self, body: &str) -> Option<CssRule> {
        let context = self.parser_context(CssRuleType::FontFace);
        let mut input = ParserInput::new(body);
        let mut parser = Parser::new(&mut input);
        let rule =
            parse_font_face_block(&context, &mut parser, SourceLocation { line: 0, column: 0 });
        Some(CssRule::FontFace(Arc::new(self.lock.wrap(rule))))
    }

    /// Parse individual declarations into one declaration block.
    fn parse_declaration_block<'d>(
        &self,
        declarations: impl IntoIterator<Item = RawDeclaration<'d>>,
        rule_type: CssRuleType,
    ) -> PropertyDeclarationBlock {
        let context = self.parser_context(rule_type);
        let mut block = PropertyDeclarationBlock::new();
        let mut source = SourcePropertyDeclaration::default();
        for (name, value, important) in declarations {
            // The gated parse (not `parse_unchecked`) so properties the lynx
            // stylo build disables stop here, exactly as in a text parse.
            let Ok(id) = PropertyId::parse(name, &context) else {
                continue;
            };
            // Drop any leftovers from a previous failed parse.
            drop(source.drain());
            if parse_one_declaration_into(
                &mut source,
                id,
                value,
                Origin::Author,
                &self.url_data,
                None,
                ParsingMode::DEFAULT,
                QuirksMode::NoQuirks,
                rule_type,
            )
            .is_ok()
            {
                let importance = if important {
                    Importance::Important
                } else {
                    Importance::Normal
                };
                block.extend(source.drain(), importance);
            }
        }
        block
    }

    /// A parser context for this engine's URL data, author origin.
    fn parser_context(&self, rule_type: CssRuleType) -> ParserContext<'_> {
        ParserContext::new(
            Origin::Author,
            &self.url_data,
            Some(rule_type),
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            std::borrow::Cow::default(),
            None,
            None,
            AttrTaint::default(),
        )
    }

    /// Whether an `@keyframes` animation with this name has been registered
    /// (via any appended stylesheet). The node picks the cascade data the
    /// lookup runs against; pass the tree root for document-level rules.
    #[must_use]
    pub fn has_keyframes_animation<T: ExternalState>(&self, name: &str, node: &Node<T>) -> bool {
        self.stylist
            .lookup_keyframes(&stylo_atoms::Atom::from(name), node)
            .is_some()
    }

    /// The number of registered `@font-face` rules across all origins.
    #[must_use]
    pub fn font_face_count(&self) -> usize {
        self.stylist
            .iter_extra_data_origins()
            .map(|(data, _)| data.font_faces.len())
            .sum()
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

    /// Match and cascade one node into standard CSS computed values.
    ///
    /// `parent_style` supplies inherited values. At a document root, pass
    /// `None` to inherit from stylo's initial values.
    /// # Panics
    ///
    /// Panics when `node` is a text node or belongs to a document not created
    /// by this engine (the same boundary check as `flush_document`).
    #[must_use]
    pub fn resolve<T: ExternalState>(
        &self,
        node: &Node<T>,
        parent_style: Option<&ComputedValues>,
    ) -> Arc<ComputedValues> {
        assert!(
            node.is_element(),
            "StyleEngine::resolve called with a text node"
        );
        assert_eq!(
            node.tree().engine_token,
            Some(self.token),
            "node's document is not paired with this StyleEngine"
        );
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
            node,
            None,
            node.style_attribute(),
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
        cascade::<&Node<T>>(
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

impl<T> Document<T> {
    /// Store a resolved style and mark the node's own style work complete.
    ///
    /// Descendant dirtiness is left intact. Used with the standalone
    /// [`StyleEngine::resolve`] path; the flush traversal
    /// ([`StyleEngine::flush_document`]) stores styles itself.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn store_computed_style(&mut self, id: NodeId, style: Arc<ComputedValues>) {
        let node = self
            .core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::store_computed_style");
        assert!(
            node.is_element(),
            "Document::store_computed_style called with a text node"
        );
        node.set_computed_style(style);
        node.set_style_dirty(false);
    }
}
