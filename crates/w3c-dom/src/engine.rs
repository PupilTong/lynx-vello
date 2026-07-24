//! Standards-oriented CSS parsing, selector matching, and cascade execution.

use std::sync::Arc as StdArc;
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
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::keyframes_rule::{KeyframesRule, parse_keyframe_list};
use stylo::stylesheets::{
    AllowImportRules, CssRule as StyloCssRule, CssRuleType, CssRules, DocumentStyleSheet, Origin,
    StyleRule, Stylesheet, StylesheetContents, UrlExtraData,
};
use stylo::stylist::{RuleInclusion, Stylist};
use stylo::values::KeyframesName;
use stylo::values::specified::position::PositionTryFallbacksTryTactic;
use stylo_traits::ParsingMode;

use crate::{Document, Node};

/// A parsed CSS declaration input used when constructing style rules directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssDeclaration<'a> {
    pub property: &'a str,
    pub value: std::borrow::Cow<'a, str>,
    pub important: bool,
}

/// One pre-built CSS rule branded with the private document context that
/// parsed and locked it.
#[derive(Clone, Debug)]
pub struct CssRule {
    inner: StyloCssRule,
    owner: StdArc<SharedRwLock>,
}

impl CssRule {
    fn new(inner: StyloCssRule, owner: &StdArc<SharedRwLock>) -> Self {
        Self {
            inner,
            owner: StdArc::clone(owner),
        }
    }
}

#[must_use]
pub fn property_is_supported(name: &str) -> bool {
    matches!(
        stylo::properties::PropertyId::parse_unchecked(name, None),
        Ok(stylo::properties::PropertyId::NonCustom(_))
    )
}

/// The private stylo state owned by exactly one [`Document`].
pub(crate) struct StyleEngine {
    stylist: Stylist,
    lock: StdArc<SharedRwLock>,
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
    #[must_use]
    pub(crate) fn with_url_data(device: Device, url_data: UrlExtraData) -> Self {
        Self {
            stylist: Stylist::new(device, QuirksMode::NoQuirks),
            lock: StdArc::new(SharedRwLock::new()),
            url_data,
        }
    }

    pub(crate) fn lock(&self) -> StdArc<SharedRwLock> {
        StdArc::clone(&self.lock)
    }

    pub(crate) fn url_data(&self) -> UrlExtraData {
        self.url_data.clone()
    }

    pub(crate) fn stylist(&self) -> &Stylist {
        &self.stylist
    }

    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    #[must_use]
    pub(crate) fn device(&self) -> &Device {
        self.stylist.device()
    }

    pub(crate) fn update_device(&mut self, update: impl FnOnce(&mut Device)) {
        update(self.stylist.device_mut());
        self.refresh_device();
    }

    pub(crate) fn set_viewport(&mut self, width: f32, height: f32) {
        self.update_device(|device| {
            let dpr = device.device_pixel_ratio().get();
            device.set_viewport_size(euclid::Size2D::new(width, height));
            device.set_device_size(euclid::Size2D::new(width * dpr, height * dpr));
        });
    }

    pub(crate) fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.update_device(|device| {
            device.set_device_pixel_ratio(euclid::Scale::new(device_pixel_ratio));
            let viewport = device.viewport_size();
            device.set_device_size(euclid::Size2D::new(
                viewport.width * device_pixel_ratio,
                viewport.height * device_pixel_ratio,
            ));
        });
    }

    pub(crate) fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        self.add_stylesheet_with_media(css, origin, "");
    }

    pub(crate) fn add_stylesheet_with_media(
        &mut self,
        css: &str,
        origin: Origin,
        media_query: &str,
    ) {
        let media = self.parse_media_list(media_query);
        let sheet = Stylesheet::from_str(
            css,
            self.url_data.clone(),
            origin,
            media,
            self.lock.as_ref().clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );
        self.install_stylesheet(sheet);
    }

    pub(crate) fn append_rules(&mut self, rules: Vec<CssRule>, origin: Origin) {
        assert!(
            rules
                .iter()
                .all(|rule| StdArc::ptr_eq(&rule.owner, &self.lock)),
            "CSS rule belongs to another Document"
        );
        let rules = rules.into_iter().map(|rule| rule.inner).collect();
        let rules = CssRules::new(rules, &self.lock);
        let contents = StylesheetContents::from_rules(
            rules,
            origin,
            self.url_data.clone(),
            QuirksMode::NoQuirks,
        );
        let sheet = Stylesheet {
            contents: self.lock.wrap(contents),
            shared_lock: self.lock.as_ref().clone(),
            media: Arc::new(self.lock.wrap(MediaList::empty())),
            disabled: AtomicBool::new(false),
        };
        self.install_stylesheet(sheet);
    }

    fn install_stylesheet(&mut self, sheet: Stylesheet) {
        let guard = self.lock.read();
        self.stylist
            .append_stylesheet(DocumentStyleSheet(Arc::new(sheet)), &guard);
        self.stylist.flush(&StylesheetGuards::same(&guard));
    }

    #[must_use]
    pub(crate) fn build_style_rule<'d>(
        &self,
        selectors: &str,
        declarations: impl IntoIterator<Item = CssDeclaration<'d>>,
    ) -> Option<CssRule> {
        let selectors =
            SelectorParser::parse_author_origin_no_namespace(selectors, &self.url_data).ok()?;
        let block = self.parse_declaration_block(declarations, CssRuleType::Style);
        Some(CssRule::new(
            StyloCssRule::Style(Arc::new(self.lock.wrap(StyleRule {
                selectors,
                block: Arc::new(self.lock.wrap(block)),
                rules: None,
                source_location: SourceLocation { line: 0, column: 0 },
            }))),
            &self.lock,
        ))
    }

    #[must_use]
    pub(crate) fn build_keyframes_rule(&self, name: &str, body: &str) -> Option<CssRule> {
        if name.is_empty() {
            return None;
        }
        let mut context = self.parser_context(CssRuleType::Keyframes);
        let mut input = ParserInput::new(body);
        let mut parser = Parser::new(&mut input);
        let keyframes = parse_keyframe_list(&mut context, &mut parser, &self.lock);
        Some(CssRule::new(
            StyloCssRule::Keyframes(Arc::new(self.lock.wrap(KeyframesRule {
                name: KeyframesName::from_ident(name),
                keyframes,
                vendor_prefix: None,
                source_location: SourceLocation { line: 0, column: 0 },
            }))),
            &self.lock,
        ))
    }

    #[must_use]
    pub(crate) fn build_font_face_rule(&self, body: &str) -> CssRule {
        let context = self.parser_context(CssRuleType::FontFace);
        let mut input = ParserInput::new(body);
        let mut parser = Parser::new(&mut input);
        let rule =
            parse_font_face_block(&context, &mut parser, SourceLocation { line: 0, column: 0 });
        CssRule::new(
            StyloCssRule::FontFace(Arc::new(self.lock.wrap(rule))),
            &self.lock,
        )
    }

    fn parse_declaration_block<'d>(
        &self,
        declarations: impl IntoIterator<Item = CssDeclaration<'d>>,
        rule_type: CssRuleType,
    ) -> PropertyDeclarationBlock {
        let context = self.parser_context(rule_type);
        let mut block = PropertyDeclarationBlock::new();
        let mut source = SourcePropertyDeclaration::default();
        for declaration in declarations {
            let Ok(id) = PropertyId::parse(declaration.property, &context) else {
                continue;
            };
            drop(source.drain());
            if parse_one_declaration_into(
                &mut source,
                id,
                declaration.value.as_ref(),
                Origin::Author,
                &self.url_data,
                None,
                ParsingMode::DEFAULT,
                QuirksMode::NoQuirks,
                rule_type,
            )
            .is_ok()
            {
                let importance = if declaration.important {
                    Importance::Important
                } else {
                    Importance::Normal
                };
                block.extend(source.drain(), importance);
            }
        }
        block
    }

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

    #[must_use]
    pub(crate) fn has_keyframes_animation<T: Sync>(&self, name: &str, node: &Node<T>) -> bool {
        self.stylist
            .lookup_keyframes(&stylo_atoms::Atom::from(name), node)
            .is_some()
    }

    #[must_use]
    pub(crate) fn font_face_count(&self) -> usize {
        self.stylist
            .iter_extra_data_origins()
            .map(|(data, _)| data.font_faces.len())
            .sum()
    }

    #[must_use]
    pub(crate) fn parse_media_list(
        &self,
        media_query: &str,
    ) -> Arc<stylo::shared_lock::Locked<MediaList>> {
        let mut input = ParserInput::new(media_query);
        let mut parser = Parser::new(&mut input);
        let mut context = self.parser_context(CssRuleType::Media);
        let media = MediaList::parse(&mut context, &mut parser);
        Arc::new(self.lock.wrap(media))
    }

    #[must_use]
    pub(crate) fn resolve_style<T: Sync>(
        &self,
        node: &Node<T>,
        parent_style: Option<&ComputedValues>,
    ) -> Arc<ComputedValues> {
        assert!(
            node.is_element(),
            "Document::resolve_style called with a text node"
        );
        assert!(
            StdArc::ptr_eq(node.document_lock(), &self.lock),
            "node does not belong to this Document"
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
    #[must_use]
    pub fn device(&self) -> &Device {
        self.style_engine().device()
    }

    pub fn update_device(&mut self, update: impl FnOnce(&mut Device)) {
        self.change_style_context(|engine| engine.update_device(update));
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.change_style_context(|engine| engine.set_viewport(width, height));
    }

    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.change_style_context(|engine| engine.set_device_pixel_ratio(device_pixel_ratio));
    }

    pub fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        self.change_style_context(|engine| engine.add_stylesheet(css, origin));
    }

    pub fn add_stylesheet_with_media(&mut self, css: &str, origin: Origin, media_query: &str) {
        self.change_style_context(|engine| {
            engine.add_stylesheet_with_media(css, origin, media_query);
        });
    }

    pub fn append_rules(&mut self, rules: Vec<CssRule>, origin: Origin) {
        self.change_style_context(|engine| engine.append_rules(rules, origin));
    }

    #[must_use]
    pub fn build_style_rule<'d>(
        &self,
        selectors: &str,
        declarations: impl IntoIterator<Item = CssDeclaration<'d>>,
    ) -> Option<CssRule> {
        self.style_engine()
            .build_style_rule(selectors, declarations)
    }

    #[must_use]
    pub fn build_keyframes_rule(&self, name: &str, body: &str) -> Option<CssRule> {
        self.style_engine().build_keyframes_rule(name, body)
    }

    #[must_use]
    pub fn build_font_face_rule(&self, body: &str) -> CssRule {
        self.style_engine().build_font_face_rule(body)
    }

    #[must_use]
    pub fn has_keyframes_animation(&self, name: &str, node: &Node<T>) -> bool
    where
        T: Sync,
    {
        self.style_engine().has_keyframes_animation(name, node)
    }

    #[must_use]
    pub fn font_face_count(&self) -> usize {
        self.style_engine().font_face_count()
    }

    #[must_use]
    pub fn resolve_style(
        &self,
        node: &Node<T>,
        parent_style: Option<&ComputedValues>,
    ) -> Arc<ComputedValues>
    where
        T: Sync,
    {
        self.style_engine().resolve_style(node, parent_style)
    }

    fn change_style_context(&mut self, change: impl FnOnce(&mut StyleEngine)) {
        change(self.style_engine_mut());
        if let Some(root) = self.root_element().map(Node::id) {
            self.mark_subtree_dirty(root);
        }
    }
}
