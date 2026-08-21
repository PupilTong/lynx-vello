//! Standards-oriented CSS parsing, selector matching, and cascade execution.

use std::borrow::Cow;
use std::sync::Arc as StdArc;
use std::sync::atomic::AtomicBool;

use cssparser::{Parser, ParserInput};
use stylo::author_styles::AuthorStyles;
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::device::Device;
use stylo::font_face::parse_font_face_block;
use stylo::media_queries::MediaList;
use stylo::parser::{Parse, ParserContext};
use stylo::properties::declaration_block::parse_one_declaration_into;
use stylo::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
    SourcePropertyDeclarationUpdate,
};
use stylo::selector_parser::SelectorParser;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock, StylesheetGuards};
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::keyframes_rule::{Keyframe, KeyframeSelectors, KeyframesRule};
use stylo::stylesheets::{
    AllowImportRules, CssRule as StyloCssRule, CssRuleType, CssRules, CustomMediaMap,
    DocumentStyleSheet, Origin, StyleRule, Stylesheet, StylesheetContents, UrlExtraData,
};
use stylo::stylist::Stylist;
use stylo::values::{KeyframesName, SourceLocation};
use stylo_traits::ParsingMode;

use crate::Document;
use crate::tree::document::NodeId;
use crate::tree::shadow::ShadowRootData;

/// One declaration of a directly constructed rule: a property name plus the
/// value text a decoded wire format already carries.
///
/// This is the pre-parsed ingestion boundary. The name is resolved to a
/// [`PropertyId`] and the value text is handed to stylo's value parser, so no
/// declaration list is ever tokenized and no property name is looked up twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssDeclaration<'a> {
    /// The CSS property name, as spelled by the source that produced it.
    pub property: &'a str,
    /// The declaration value text, without the trailing `!important`.
    pub value: Cow<'a, str>,
    /// Whether the declaration carries `!important`.
    pub important: bool,
}

/// One `@keyframes` child block of a directly constructed keyframes rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssKeyframe<'a> {
    /// The keyframe selector text (`from`, `to`, `50%`, or a comma list).
    pub selector: &'a str,
    /// The declarations of this keyframe block.
    pub declarations: Vec<CssDeclaration<'a>>,
}

/// One pre-built CSS rule, branded with the document context that parsed and
/// locked it.
///
/// The inner stylo rule and the [`SharedRwLock`] that guards it never leave
/// this type, so a rule can only be appended to the document that built it.
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
    pub(crate) fn new(device: Device, url_data: UrlExtraData) -> Self {
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

    /// Applies one CSSOM-style property update to an inline declaration block.
    ///
    /// Returning `None` means that the property name or value was invalid, or
    /// that the requested update would not change the declaration block. An
    /// empty value removes the property, matching
    /// `CSSStyleDeclaration.setProperty`.
    pub(crate) fn update_inline_style_property(
        &self,
        existing: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
        property: &str,
        value: &str,
    ) -> Option<(Option<Arc<Locked<PropertyDeclarationBlock>>>, String)> {
        let context = self.parser_context(CssRuleType::Style);
        let property = PropertyId::parse(property, &context).ok()?;
        let mut block = existing.map_or_else(PropertyDeclarationBlock::new, |existing| {
            let guard = self.lock.read();
            existing.read_with(&guard).clone()
        });

        if value.is_empty() {
            let first = block.first_declaration_to_remove(&property)?;
            block.remove_property(&property, first);
        } else {
            let mut source = SourcePropertyDeclaration::default();
            parse_one_declaration_into(
                &mut source,
                property,
                value,
                Origin::Author,
                &self.url_data,
                None,
                ParsingMode::DEFAULT,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            )
            .ok()?;

            let mut updates = SourcePropertyDeclarationUpdate::default();
            if !block.prepare_for_update(&source, Importance::Normal, &mut updates) {
                return None;
            }
            block.update(source.drain(), Importance::Normal, &mut updates);
        }

        let mut css = String::new();
        block
            .to_css(&mut css)
            .expect("serializing a declaration block into a String cannot fail");
        let block = (!block.is_empty()).then(|| Arc::new(self.lock.wrap(block)));
        Some((block, css))
    }

    /// Builds a complete inline declaration block from a record of
    /// name/value pairs.
    ///
    /// This is the batch counterpart of
    /// [`Self::update_inline_style_property`], for the setter whose semantics
    /// are a *replacement* rather than a mutation. Building from an empty
    /// block is what makes it linear: the per-property path has to clone and
    /// re-serialize the whole block for each declaration, so replaying a
    /// record of `n` declarations through it costs `O(n²)`.
    ///
    /// Each declaration is parsed exactly as
    /// [`Self::update_inline_style_property`] parses one — a name that does
    /// not resolve, or a value that does not parse, drops that declaration
    /// and nothing else. An empty value is skipped: `setProperty` with an
    /// empty value removes a property, and there is nothing here to remove.
    ///
    /// Returns the block (`None` when nothing parsed) and its serialization,
    /// which is what the `style` attribute reads back as.
    pub(crate) fn build_inline_style_block<'a>(
        &self,
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> (Option<Arc<Locked<PropertyDeclarationBlock>>>, String) {
        let context = self.parser_context(CssRuleType::Style);
        let mut block = PropertyDeclarationBlock::new();
        let mut source = SourcePropertyDeclaration::default();
        for (property, value) in declarations {
            // Fresh per declaration: `prepare_for_update` asserts it starts
            // empty and only `update` clears it, so a declaration that
            // parses but changes nothing would poison the next one.
            let mut updates = SourcePropertyDeclarationUpdate::default();
            if value.is_empty() {
                continue;
            }
            let Ok(property) = PropertyId::parse(property, &context) else {
                continue;
            };
            source.clear();
            if parse_one_declaration_into(
                &mut source,
                property,
                value,
                Origin::Author,
                &self.url_data,
                None,
                ParsingMode::DEFAULT,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            )
            .is_err()
            {
                continue;
            }
            if !block.prepare_for_update(&source, Importance::Normal, &mut updates) {
                continue;
            }
            block.update(source.drain(), Importance::Normal, &mut updates);
        }

        let mut css = String::new();
        block
            .to_css(&mut css)
            .expect("serializing a declaration block into a String cannot fail");
        let block = (!block.is_empty()).then(|| Arc::new(self.lock.wrap(block)));
        (block, css)
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

    fn parse_stylesheet(&self, css: &str, origin: Origin) -> DocumentStyleSheet {
        let media = Arc::new(self.lock.wrap(MediaList::empty()));
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
        DocumentStyleSheet(Arc::new(sheet))
    }

    pub(crate) fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        let sheet = self.parse_stylesheet(css, origin);
        let guard = self.lock.read();
        self.stylist.append_stylesheet(sheet, &guard);
        self.stylist.flush(&StylesheetGuards::same(&guard));
    }

    pub(crate) fn add_scoped_stylesheet(
        &mut self,
        styles: &mut AuthorStyles<DocumentStyleSheet>,
        css: &str,
    ) {
        let sheet = self.parse_stylesheet(css, Origin::Author);
        let guard = self.lock.read();
        styles
            .stylesheets
            .append_stylesheet(None, &CustomMediaMap::default(), sheet, &guard);
        drop(styles.flush(&mut self.stylist, &guard));
    }

    /// Installs rules this engine built as one author-origin sheet.
    pub(crate) fn append_rules(&mut self, rules: Vec<CssRule>) {
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
            Origin::Author,
            self.url_data.clone(),
            QuirksMode::NoQuirks,
        );
        let sheet = Stylesheet {
            contents: self.lock.wrap(contents),
            shared_lock: self.lock.as_ref().clone(),
            media: Arc::new(self.lock.wrap(MediaList::empty())),
            disabled: AtomicBool::new(false),
        };
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
    pub(crate) fn build_keyframes_rule<'d>(
        &self,
        name: &str,
        keyframes: impl IntoIterator<Item = CssKeyframe<'d>>,
    ) -> Option<CssRule> {
        // `<keyframes-name>` is an ident *or* a string, and a producer that
        // preserves the authored prelude hands over the quotes with it. Parsing
        // resolves both spellings to the same atom, which is what
        // `animation-name` is matched against; taking the text as an ident
        // would leave `"spin"` unable to match `spin`, and would make the names
        // that only exist in string form (`"none"`) unreachable.
        let context = self.parser_context(CssRuleType::Keyframes);
        let mut input = ParserInput::new(name);
        let name = Parser::new(&mut input)
            .parse_entirely(|input| KeyframesName::parse(&context, input))
            .ok()?;
        let keyframes = keyframes
            .into_iter()
            .filter_map(|keyframe| {
                let mut input = ParserInput::new(keyframe.selector);
                let selector = KeyframeSelectors::parse(&mut Parser::new(&mut input)).ok()?;
                let block =
                    self.parse_declaration_block(keyframe.declarations, CssRuleType::Keyframe);
                Some(Arc::new(self.lock.wrap(Keyframe {
                    selector,
                    block: Arc::new(self.lock.wrap(block)),
                    source_location: SourceLocation { line: 0, column: 0 },
                })))
            })
            .collect();
        Some(CssRule::new(
            StyloCssRule::Keyframes(Arc::new(self.lock.wrap(KeyframesRule {
                name,
                keyframes,
                vendor_prefix: None,
                source_location: SourceLocation { line: 0, column: 0 },
            }))),
            &self.lock,
        ))
    }

    /// Builds an `@font-face` rule from its descriptor block text.
    ///
    /// Font-face descriptors are not CSS properties, so stylo exposes no
    /// per-descriptor constructor and this one block is parsed as text. Its
    /// input is a single rule body, never a stylesheet.
    #[must_use]
    pub(crate) fn build_font_face_rule(&self, descriptors: &str) -> CssRule {
        let context = self.parser_context(CssRuleType::FontFace);
        let mut input = ParserInput::new(descriptors);
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
            Cow::default(),
            None,
            None,
            AttrTaint::default(),
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
    pub(crate) fn device(&self) -> &Device {
        self.style_engine().device()
    }

    /// The viewport in CSS px.
    #[must_use]
    pub fn viewport_size(&self) -> crate::Size2D<f32> {
        let size = self.device().viewport_size();
        crate::Size2D::new(size.width, size.height)
    }

    /// CSS-px → device-px scale factor.
    #[must_use]
    pub fn device_pixel_ratio(&self) -> f32 {
        self.device().device_pixel_ratio().get()
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

    /// Appends rules this document built as one author-origin stylesheet.
    ///
    /// This is the pre-parsed ingestion path: the caller lowers an already
    /// decoded wire format through [`Document::build_style_rule`] and friends,
    /// so no CSS stylesheet text is produced or tokenized. Later calls cascade
    /// over earlier ones, as later stylesheets do in a document.
    ///
    /// The origin is fixed rather than chosen, because the builders resolve
    /// property names and values in an author parser context; a sheet mounted
    /// at another origin would not be the sheet that was parsed.
    ///
    /// # Panics
    ///
    /// Panics if any rule was built by a different document.
    pub fn append_rules(&mut self, rules: Vec<CssRule>) {
        self.change_style_context(|engine| engine.append_rules(rules));
    }

    /// Builds one style rule from selector text and pre-parsed declarations.
    ///
    /// Returns `None` if the selector list does not parse; individual
    /// declarations that do not parse are dropped, as in a stylesheet.
    #[must_use]
    pub fn build_style_rule<'d>(
        &self,
        selectors: &str,
        declarations: impl IntoIterator<Item = CssDeclaration<'d>>,
    ) -> Option<CssRule> {
        self.style_engine()
            .build_style_rule(selectors, declarations)
    }

    /// Builds one `@keyframes` rule from its name and its keyframe blocks.
    ///
    /// Returns `None` for an empty name; keyframes whose selector does not
    /// parse are dropped.
    #[must_use]
    pub fn build_keyframes_rule<'d>(
        &self,
        name: &str,
        keyframes: impl IntoIterator<Item = CssKeyframe<'d>>,
    ) -> Option<CssRule> {
        self.style_engine().build_keyframes_rule(name, keyframes)
    }

    /// Builds one `@font-face` rule from its descriptor block text.
    #[must_use]
    pub fn build_font_face_rule(&self, descriptors: &str) -> CssRule {
        self.style_engine().build_font_face_rule(descriptors)
    }

    /// Adds an author stylesheet scoped to one shadow tree.
    pub fn add_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_shadow_stylesheet: not a live shadow root");
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.add_scoped_stylesheet(&mut shadow.styles, css);
        }
        self.mark_subtree_dirty(host);
    }

    fn shadow_style_parts(
        &mut self,
        shadow_root: NodeId,
    ) -> (&mut StyleEngine, &mut ShadowRootData) {
        let (engine, tree) = self.style_and_tree_parts();
        let shadow = tree
            .get_mut(shadow_root)
            .expect("stale NodeId passed to a shadow-root method")
            .shadow_data_mut()
            .expect("Document shadow methods take a shadow root");
        (engine, shadow)
    }

    fn change_style_context(&mut self, change: impl FnOnce(&mut StyleEngine)) {
        self.note_visual_mutation();
        change(self.style_engine_mut());
        let root = self.document_element().id();
        self.mark_subtree_dirty(root);
    }
}

#[cfg(test)]
mod tests {
    use stylo_traits::ToCss;

    use super::*;
    use crate::tree::document::tests::device;

    fn document() -> Document<()> {
        Document::<()>::new(device(), "page", ())
    }

    fn declaration<'a>(property: &'a str, value: &'a str) -> CssDeclaration<'a> {
        CssDeclaration {
            property,
            value: Cow::Borrowed(value),
            important: false,
        }
    }

    /// Nothing above this crate can observe the keyframes registry, so the
    /// name atom and the per-block selector parse are checked here, where the
    /// built rule is still readable.
    #[test]
    fn a_built_keyframes_rule_carries_its_name_selectors_and_blocks() {
        let document = document();
        let rule = document
            .build_keyframes_rule(
                "spin",
                [
                    CssKeyframe {
                        selector: "from",
                        declarations: vec![declaration("opacity", "0")],
                    },
                    CssKeyframe {
                        selector: "50%, 75%",
                        declarations: vec![declaration("opacity", "0.5")],
                    },
                    CssKeyframe {
                        selector: "to",
                        declarations: vec![declaration("opacity", "1")],
                    },
                ],
            )
            .expect("a named keyframes rule");

        let engine = document.style_engine();
        let guard = engine.shared_lock().read();
        let StyloCssRule::Keyframes(keyframes) = &rule.inner else {
            panic!("a keyframes rule");
        };
        let keyframes = keyframes.read_with(&guard);
        assert_eq!(keyframes.name.as_atom().to_string(), "spin");

        let selectors: Vec<String> = keyframes
            .keyframes
            .iter()
            .map(|keyframe| keyframe.read_with(&guard).selector.to_css_string())
            .collect();
        assert_eq!(selectors, ["0%", "50%, 75%", "100%"]);

        let declarations = keyframes.keyframes[1]
            .read_with(&guard)
            .block
            .read_with(&guard)
            .len();
        assert_eq!(declarations, 1, "each keyframe keeps its own block");
    }

    /// A keyframe whose offset does not parse is dropped; the rest survive.
    #[test]
    fn an_unparsable_keyframe_selector_drops_only_its_own_block() {
        let document = document();
        let rule = document
            .build_keyframes_rule(
                "slide",
                [
                    CssKeyframe {
                        selector: "not-an-offset",
                        declarations: vec![declaration("opacity", "0")],
                    },
                    CssKeyframe {
                        selector: "to",
                        declarations: vec![declaration("opacity", "1")],
                    },
                ],
            )
            .expect("a named keyframes rule");

        let engine = document.style_engine();
        let guard = engine.shared_lock().read();
        let StyloCssRule::Keyframes(keyframes) = &rule.inner else {
            panic!("a keyframes rule");
        };
        assert_eq!(keyframes.read_with(&guard).keyframes.len(), 1);
    }

    /// `<keyframes-name>` accepts a string as well as an ident, and both
    /// spellings must reach the same atom `animation-name` is matched against.
    #[test]
    fn a_quoted_keyframes_name_resolves_to_the_same_atom_as_the_ident() {
        let document = document();
        for spelling in ["spin", "\"spin\"", "'spin'"] {
            let rule = document
                .build_keyframes_rule(spelling, std::iter::empty::<CssKeyframe<'_>>())
                .unwrap_or_else(|| panic!("{spelling} is a valid keyframes name"));
            let engine = document.style_engine();
            let guard = engine.shared_lock().read();
            let StyloCssRule::Keyframes(keyframes) = &rule.inner else {
                panic!("a keyframes rule");
            };
            assert_eq!(
                keyframes.read_with(&guard).name.as_atom().to_string(),
                "spin",
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_keyframes_name_that_is_not_a_name_is_refused() {
        let document = document();
        for spelling in ["", "none", "\"\"", "50%", "a b"] {
            assert!(
                document
                    .build_keyframes_rule(spelling, std::iter::empty::<CssKeyframe<'_>>())
                    .is_none(),
                "{spelling:?} is not a <keyframes-name>"
            );
        }
    }

    /// A rule built by one document must never enter another's cascade: the
    /// lock that guards its contents belongs to the document that minted it.
    #[test]
    #[should_panic(expected = "CSS rule belongs to another Document")]
    fn a_rule_from_another_document_is_refused() {
        let other = document();
        let rule = other
            .build_style_rule(".a", [declaration("width", "1px")])
            .expect("a style rule");
        document().append_rules(vec![rule]);
    }
}
