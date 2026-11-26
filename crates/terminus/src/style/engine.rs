//! Standards-oriented CSS parsing, selector matching, and cascade execution.

use std::borrow::Cow;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc as StdArc, LazyLock};

use cssparser::Parser;
use rustc_hash::FxHashSet;
use stylo::author_styles::AuthorStyles;
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::device::Device;
use stylo::font_face::{FontFaceRule, Source as StyloFontFaceSource, parse_font_face_block};
use stylo::invalidation::stylesheets::StylesheetInvalidationSet;
use stylo::media_queries::MediaList;
use stylo::parser::{Parse, ParserContext};
use stylo::properties::declaration_block::parse_one_declaration_into;
use stylo::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
    SourcePropertyDeclarationUpdate,
};
use stylo::selector_parser::SelectorParser;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock, SharedRwLockReadGuard, StylesheetGuards};
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::keyframes_rule::{Keyframe, KeyframeSelectors, KeyframesRule};
use stylo::stylesheets::scope_rule::ImplicitScopeRoot;
use stylo::stylesheets::{
    AllowImportRules, CssRule as StyloCssRule, CssRuleType, CssRules, DocumentStyleSheet, Origin,
    StyleRule, Stylesheet, StylesheetContents, StylesheetInDocument, UrlExtraData,
};
use stylo::stylist::Stylist;
use stylo::values::{KeyframesName, SourceLocation};
use stylo_atoms::Atom;
use stylo_traits::ParsingMode;

use crate::tree::document::{Document, NodeId};
use crate::tree::shadow::ShadowRootData;

#[derive(Clone, Debug)]
pub(crate) struct ShadowStyleSheet(pub(crate) Arc<Stylesheet>);

impl PartialEq for ShadowStyleSheet {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl StylesheetInDocument for ShadowStyleSheet {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn media<'a>(&'a self, guard: &'a SharedRwLockReadGuard) -> Option<&'a MediaList> {
        self.0.media(guard)
    }

    fn contents<'a>(&'a self, guard: &'a SharedRwLockReadGuard) -> &'a StylesheetContents {
        self.0.contents(guard)
    }

    fn implicit_scope_root(&self) -> Option<ImplicitScopeRoot> {
        Some(ImplicitScopeRoot::Constructed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssDeclaration<'a> {
    pub property: &'a str,
    pub value: Cow<'a, str>,
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssKeyframe<'a> {
    pub selector: &'a str,
    pub declarations: Vec<CssDeclaration<'a>>,
}

static USER_AGENT_LOCK: LazyLock<SharedRwLock> = LazyLock::new(SharedRwLock::new);

pub(crate) struct RuleGuards<'a> {
    user_agent: SharedRwLockReadGuard<'static>,
    author: SharedRwLockReadGuard<'a>,
}

impl RuleGuards<'_> {
    pub(crate) fn guards(&self) -> StylesheetGuards<'_> {
        StylesheetGuards {
            ua_or_user: &self.user_agent,
            author: &self.author,
        }
    }

    fn for_origin(&self, origin: Origin) -> &SharedRwLockReadGuard<'_> {
        match origin {
            Origin::UserAgent | Origin::User => &self.user_agent,
            Origin::Author => &self.author,
        }
    }
}

pub(crate) fn rule_guards(author: &SharedRwLock) -> RuleGuards<'_> {
    RuleGuards {
        user_agent: USER_AGENT_LOCK.read(),
        author: author.read(),
    }
}

pub(crate) fn author_parser_context(
    url_data: &UrlExtraData,
    rule_type: CssRuleType,
) -> ParserContext<'_> {
    ParserContext::new(
        Origin::Author,
        url_data,
        Some(rule_type),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Cow::default(),
        None,
        None,
        AttrTaint::default(),
    )
}

#[derive(Clone, Debug)]
pub struct UserAgentStylesheet(Arc<Stylesheet>);

impl UserAgentStylesheet {
    #[must_use]
    pub fn parse(css: &str) -> Self {
        Self(Arc::new(parse_stylesheet_under(
            &USER_AGENT_LOCK,
            css,
            Origin::UserAgent,
            crate::tree::document::about_blank_url_data(),
        )))
    }
}

fn parse_stylesheet_under(
    lock: &SharedRwLock,
    css: &str,
    origin: Origin,
    url_data: UrlExtraData,
) -> Stylesheet {
    let media = Arc::new(lock.wrap(MediaList::empty()));
    Stylesheet::from_str(
        css,
        url_data,
        origin,
        media,
        lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::Yes,
    )
}

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

#[derive(Clone, Debug)]
pub struct ShadowSheet {
    inner: ShadowStyleSheet,
    owner: StdArc<SharedRwLock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoStyleUpdate {
    Invalid,
    Unchanged,
}

#[expect(
    clippy::large_enum_variant,
    reason = "one per call, on the stack; boxing the declarations would allocate on every write"
)]
pub(crate) enum PropertyUpdate {
    Remove {
        property: PropertyId,
        first: usize,
    },
    Set {
        source: SourcePropertyDeclaration,
        updates: SourcePropertyDeclarationUpdate,
    },
}

impl PropertyUpdate {
    fn apply(self, block: &mut PropertyDeclarationBlock) {
        match self {
            Self::Remove { property, first } => block.remove_property(&property, first),
            Self::Set {
                mut source,
                mut updates,
            } => block.update(source.drain(), Importance::Normal, &mut updates),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use]
pub(crate) enum StyleInvalidation {
    Recascade,
    Rematch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFaceRequest {
    pub family: String,
    pub sources: Vec<FontFaceSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFaceSource {
    Url(String),
    Local(String),
}

fn font_face_request(rule: &FontFaceRule) -> Option<FontFaceRequest> {
    let family = rule.descriptors.font_family.as_ref()?;
    let sources = rule
        .descriptors
        .src
        .as_ref()?
        .0
        .iter()
        .filter_map(|source| match source {
            StyloFontFaceSource::Url(url) => {
                let url = url.url.as_str();
                (!url.is_empty()).then(|| FontFaceSource::Url(url.to_owned()))
            }
            StyloFontFaceSource::Local(name) => Some(FontFaceSource::Local(name.name.to_string())),
        })
        .collect::<Vec<_>>();
    (!sources.is_empty()).then(|| FontFaceRequest {
        family: family.name.to_string(),
        sources,
    })
}

pub(crate) struct StyleEngine {
    stylist: Stylist,
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
    font_faces_dirty: bool,
    reported_font_faces: Vec<Arc<Locked<FontFaceRule>>>,
    changed_position_try_names: FxHashSet<Atom>,
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
            font_faces_dirty: false,
            reported_font_faces: Vec::new(),
            changed_position_try_names: FxHashSet::default(),
        }
    }

    fn note_flush(pending: &mut FxHashSet<Atom>, invalidations: &StylesheetInvalidationSet) {
        let changed = &invalidations
            .cascade_data_difference
            .changed_position_try_names;
        pending.extend(changed.iter().cloned());
    }

    pub(crate) fn take_changed_position_try_names(&mut self) -> FxHashSet<Atom> {
        std::mem::take(&mut self.changed_position_try_names)
    }

    pub(crate) fn take_font_face_requests(&mut self) -> Vec<FontFaceRequest> {
        if !std::mem::take(&mut self.font_faces_dirty) {
            return Vec::new();
        }
        let guards = rule_guards(&self.lock);
        let invalidations = self.stylist.flush(&guards.guards());
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
        let mut requests = Vec::new();
        let mut reported = Vec::new();
        for (data, origin) in self.stylist.iter_extra_data_origins() {
            for (rule, _) in data.font_faces.iter() {
                if self
                    .reported_font_faces
                    .iter()
                    .any(|seen| Arc::ptr_eq(seen, rule))
                {
                    continue;
                }
                reported.push(rule.clone());
                if let Some(request) = font_face_request(rule.read_with(guards.for_origin(origin)))
                {
                    requests.push(request);
                }
            }
        }
        drop(guards);
        self.reported_font_faces.extend(reported);
        requests
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

    pub(crate) fn rule_guards(&self) -> RuleGuards<'_> {
        rule_guards(&self.lock)
    }

    pub(crate) fn supports_property(&self, property: &str) -> bool {
        PropertyId::parse(property, &self.parser_context(CssRuleType::Style)).is_ok()
    }

    pub(crate) fn parse_style_update(
        &self,
        block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
        property: &str,
        value: &str,
        mode: ParsingMode,
    ) -> Result<PropertyUpdate, NoStyleUpdate> {
        let context = self.parser_context(CssRuleType::Style);
        let property =
            PropertyId::parse(property, &context).map_err(|()| NoStyleUpdate::Invalid)?;
        if value.is_empty() {
            return self
                .removal_update(block, &property)
                .ok_or(NoStyleUpdate::Unchanged);
        }
        let mut source = SourcePropertyDeclaration::default();
        parse_one_declaration_into(
            &mut source,
            property,
            value,
            Origin::Author,
            &self.url_data,
            None,
            mode,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        )
        .map_err(|()| NoStyleUpdate::Invalid)?;
        self.declaration_update(block, source)
            .ok_or(NoStyleUpdate::Unchanged)
    }

    pub(crate) fn removal_update(
        &self,
        block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
        property: &PropertyId,
    ) -> Option<PropertyUpdate> {
        let first = block?
            .read_with(&self.lock.read())
            .first_declaration_to_remove(property)?;
        Some(PropertyUpdate::Remove {
            property: property.clone(),
            first,
        })
    }

    pub(crate) fn declaration_update(
        &self,
        block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
        source: SourcePropertyDeclaration,
    ) -> Option<PropertyUpdate> {
        let guard = self.lock.read();
        let mut updates = SourcePropertyDeclarationUpdate::default();
        let empty;
        let current = if let Some(block) = block {
            block.read_with(&guard)
        } else {
            empty = PropertyDeclarationBlock::new();
            &empty
        };
        current
            .prepare_for_update(&source, Importance::Normal, &mut updates)
            .then_some(PropertyUpdate::Set { source, updates })
    }

    pub(crate) fn apply_style_update(
        &self,
        slot: &mut Option<Arc<Locked<PropertyDeclarationBlock>>>,
        update: PropertyUpdate,
    ) {
        if let Some(block) = slot.as_ref().filter(|block| block.is_unique()) {
            let mut guard = self.lock.write();
            let declarations = block.write_with(&mut guard);
            update.apply(declarations);
            if declarations.is_empty() {
                drop(guard);
                *slot = None;
            }
            return;
        }
        let mut block = slot
            .as_ref()
            .map_or_else(PropertyDeclarationBlock::new, |shared| {
                let guard = self.lock.read();
                shared.read_with(&guard).clone()
            });
        update.apply(&mut block);
        *slot = (!block.is_empty()).then(|| Arc::new(self.lock.wrap(block)));
    }

    pub(crate) fn build_inline_style_block<'a>(
        &self,
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        let context = self.parser_context(CssRuleType::Style);
        let mut block = PropertyDeclarationBlock::new();
        let mut source = SourcePropertyDeclaration::default();
        for (property, value) in declarations {
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

        (!block.is_empty()).then(|| Arc::new(self.lock.wrap(block)))
    }

    #[must_use]
    pub(crate) fn device(&self) -> &Device {
        self.stylist.device()
    }

    pub(crate) fn update_device(&mut self, update: impl FnOnce(&mut Device)) -> StyleInvalidation {
        update(self.stylist.device_mut());
        self.refresh_device()
    }

    pub(crate) fn set_viewport(&mut self, width: f32, height: f32) -> StyleInvalidation {
        self.update_device(|device| {
            let dpr = device.device_pixel_ratio().get();
            device.set_viewport_size(euclid::Size2D::new(width, height));
            device.set_device_size(euclid::Size2D::new(width * dpr, height * dpr));
        })
    }

    pub(crate) fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) -> StyleInvalidation {
        self.update_device(|device| {
            device.set_device_pixel_ratio(euclid::Scale::new(device_pixel_ratio));
            let viewport = device.viewport_size();
            device.set_device_size(euclid::Size2D::new(
                viewport.width * device_pixel_ratio,
                viewport.height * device_pixel_ratio,
            ));
        })
    }

    fn parse_stylesheet(&self, css: &str, origin: Origin) -> Stylesheet {
        self.parse_stylesheet_at(css, origin, self.url_data.clone())
    }

    fn parse_stylesheet_at(&self, css: &str, origin: Origin, url_data: UrlExtraData) -> Stylesheet {
        let lock = match origin {
            Origin::UserAgent | Origin::User => &*USER_AGENT_LOCK,
            Origin::Author => self.lock.as_ref(),
        };
        parse_stylesheet_under(lock, css, origin, url_data)
    }

    pub(crate) fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        let sheet = DocumentStyleSheet(Arc::new(self.parse_stylesheet(css, origin)));
        self.mount_document_sheet(sheet, origin);
    }

    pub(crate) fn add_user_agent_stylesheet(&mut self, sheet: &UserAgentStylesheet) {
        self.mount_document_sheet(DocumentStyleSheet(Arc::clone(&sheet.0)), Origin::UserAgent);
    }

    fn mount_document_sheet(&mut self, sheet: DocumentStyleSheet, origin: Origin) {
        let guards = rule_guards(&self.lock);
        self.stylist
            .append_stylesheet(sheet, guards.for_origin(origin));
        let invalidations = self.stylist.flush(&guards.guards());
        drop(guards);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
        self.font_faces_dirty = true;
    }

    fn parse_scoped_stylesheet(&self, css: &str) -> ShadowSheet {
        self.shadow_sheet(self.parse_stylesheet(css, Origin::Author))
    }

    fn parse_engine_scoped_stylesheet(&self, css: &str) -> ShadowSheet {
        self.shadow_sheet(self.parse_stylesheet_at(
            css,
            Origin::Author,
            crate::tree::document::engine_url_data(),
        ))
    }

    fn shadow_sheet(&self, sheet: Stylesheet) -> ShadowSheet {
        ShadowSheet {
            inner: ShadowStyleSheet(Arc::new(sheet)),
            owner: StdArc::clone(&self.lock),
        }
    }

    fn add_scoped_sheet(
        &mut self,
        styles: &mut AuthorStyles<ShadowStyleSheet>,
        sheet: &ShadowSheet,
    ) {
        assert!(
            StdArc::ptr_eq(&sheet.owner, &self.lock),
            "ShadowSheet belongs to another Document"
        );
        let sheet = sheet.inner.clone();
        assert!(
            !styles.stylesheets.iter().any(|mounted| *mounted == sheet),
            "a ShadowSheet is mounted at most once per shadow tree"
        );
        let guard = self.lock.read();
        styles.stylesheets.append_stylesheet(sheet, &guard);
        let invalidations = styles.flush(&mut self.stylist, &guard);
        drop(guard);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
    }

    pub(crate) fn replace_scoped_stylesheet(
        &mut self,
        styles: &mut AuthorStyles<ShadowStyleSheet>,
        index: usize,
        css: &str,
    ) {
        let sheet = ShadowStyleSheet(Arc::new(self.parse_stylesheet(css, Origin::Author)));
        let old = styles
            .stylesheets
            .get(index)
            .cloned()
            .expect("Document::replace_shadow_stylesheet: no sheet at that index");
        let guard = self.lock.read();
        styles
            .stylesheets
            .insert_stylesheet_before(sheet, old.clone(), &guard);
        let data = styles.data.clone();
        styles
            .stylesheets
            .remove_stylesheet(None, data.custom_media_map(), old, &guard);
        let invalidations = styles.flush(&mut self.stylist, &guard);
        drop(guard);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
    }

    fn stylesheet_from_rules(&self, rules: Vec<CssRule>) -> Stylesheet {
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
        Stylesheet {
            contents: self.lock.wrap(contents),
            shared_lock: self.lock.as_ref().clone(),
            media: Arc::new(self.lock.wrap(MediaList::empty())),
            disabled: AtomicBool::new(false),
        }
    }

    pub(crate) fn append_scoped_rules(
        &mut self,
        styles: &mut AuthorStyles<ShadowStyleSheet>,
        rules: Vec<CssRule>,
    ) {
        let sheet = ShadowStyleSheet(Arc::new(self.stylesheet_from_rules(rules)));
        let guard = self.lock.read();
        styles.stylesheets.append_stylesheet(sheet, &guard);
        let invalidations = styles.flush(&mut self.stylist, &guard);
        drop(guard);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
    }

    pub(crate) fn clear_scoped_stylesheets(&mut self, styles: &mut AuthorStyles<ShadowStyleSheet>) {
        let sheets: Vec<ShadowStyleSheet> = styles.stylesheets.iter().cloned().collect();
        let guard = self.lock.read();
        for sheet in sheets {
            styles.stylesheets.remove_stylesheet(
                None,
                styles.data.custom_media_map(),
                sheet,
                &guard,
            );
        }
        let invalidations = styles.flush(&mut self.stylist, &guard);
        drop(guard);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
    }

    pub(crate) fn append_rules(&mut self, rules: Vec<CssRule>) {
        let sheet = self.stylesheet_from_rules(rules);
        self.mount_document_sheet(DocumentStyleSheet(Arc::new(sheet)), Origin::Author);
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
        let context = self.parser_context(CssRuleType::Keyframes);
        let name = Parser::new(name)
            .parse_entirely(|input| KeyframesName::parse(&context, input))
            .ok()?;
        let keyframes = keyframes
            .into_iter()
            .filter_map(|keyframe| {
                let selector =
                    KeyframeSelectors::parse(&mut Parser::new(keyframe.selector)).ok()?;
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

    #[must_use]
    pub(crate) fn build_font_face_rule(&self, descriptors: &str) -> CssRule {
        let context = self.parser_context(CssRuleType::FontFace);
        let mut parser = Parser::new(descriptors);
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
        author_parser_context(&self.url_data, rule_type)
    }

    fn refresh_device(&mut self) -> StyleInvalidation {
        let guards = rule_guards(&self.lock);
        let changed = self
            .stylist
            .media_features_change_changed_style(&guards.guards(), self.stylist.device());
        if changed.is_empty() {
            return StyleInvalidation::Recascade;
        }
        self.stylist.force_stylesheet_origins_dirty(changed);
        let invalidations = self.stylist.flush(&guards.guards());
        drop(guards);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
        StyleInvalidation::Rematch
    }
}

impl<T> Document<T> {
    #[must_use]
    pub(crate) fn device(&self) -> &Device {
        self.style_engine().device()
    }

    #[must_use]
    pub fn viewport_size(&self) -> euclid::default::Size2D<f32> {
        let size = self.device().viewport_size();
        euclid::default::Size2D::new(size.width, size.height)
    }

    #[must_use]
    pub fn device_pixel_ratio(&self) -> f32 {
        self.device().device_pixel_ratio().get()
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.change_device(|engine| engine.set_viewport(width, height));
    }

    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.change_device(|engine| engine.set_device_pixel_ratio(device_pixel_ratio));
    }

    pub fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        self.change_style_rules(|engine| engine.add_stylesheet(css, origin));
    }

    pub fn add_user_agent_stylesheet(&mut self, sheet: &UserAgentStylesheet) {
        self.change_style_rules(|engine| engine.add_user_agent_stylesheet(sheet));
    }

    pub fn append_rules(&mut self, rules: Vec<CssRule>) {
        self.change_style_rules(|engine| engine.append_rules(rules));
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
    pub fn build_keyframes_rule<'d>(
        &self,
        name: &str,
        keyframes: impl IntoIterator<Item = CssKeyframe<'d>>,
    ) -> Option<CssRule> {
        self.style_engine().build_keyframes_rule(name, keyframes)
    }

    #[must_use]
    pub fn build_font_face_rule(&self, descriptors: &str) -> CssRule {
        self.style_engine().build_font_face_rule(descriptors)
    }

    pub fn add_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_shadow_stylesheet: not a live shadow root");
        let sheet = self.style_engine().parse_scoped_stylesheet(css);
        self.mount_shadow_sheet(shadow_root, host, &sheet);
    }

    #[must_use]
    pub fn parse_shadow_stylesheet(&self, css: &str) -> ShadowSheet {
        self.style_engine().parse_scoped_stylesheet(css)
    }

    #[must_use]
    pub fn parse_engine_shadow_stylesheet(&self, css: &str) -> ShadowSheet {
        self.style_engine().parse_engine_scoped_stylesheet(css)
    }

    pub fn add_shadow_sheet(&mut self, shadow_root: NodeId, sheet: &ShadowSheet) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_shadow_sheet: not a live shadow root");
        self.mount_shadow_sheet(shadow_root, host, sheet);
    }

    fn mount_shadow_sheet(&mut self, shadow_root: NodeId, host: NodeId, sheet: &ShadowSheet) {
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.add_scoped_sheet(&mut shadow.styles, sheet);
        }
        self.mark_subtree_dirty(host);
    }

    pub fn add_engine_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_engine_shadow_stylesheet: not a live shadow root");
        let sheet = self.style_engine().parse_engine_scoped_stylesheet(css);
        self.mount_shadow_sheet(shadow_root, host, &sheet);
    }

    pub fn replace_shadow_stylesheet(&mut self, shadow_root: NodeId, index: usize, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::replace_shadow_stylesheet: not a live shadow root");
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.replace_scoped_stylesheet(&mut shadow.styles, index, css);
        }
        self.mark_subtree_dirty(host);
    }

    pub fn append_shadow_rules(&mut self, shadow_root: NodeId, rules: Vec<CssRule>) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::append_shadow_rules: not a live shadow root");
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.append_scoped_rules(&mut shadow.styles, rules);
        }
        self.mark_subtree_dirty(host);
    }

    pub fn clear_shadow_stylesheets(&mut self, shadow_root: NodeId) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::clear_shadow_stylesheets: not a live shadow root");
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.clear_scoped_stylesheets(&mut shadow.styles);
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

    fn change_device(&mut self, change: impl FnOnce(&mut StyleEngine) -> StyleInvalidation) {
        self.note_visual_mutation();
        let invalidation = change(self.style_engine_mut());
        let root = self.document_element().id();
        match invalidation {
            StyleInvalidation::Rematch => self.mark_subtree_dirty(root),
            StyleInvalidation::Recascade => self.mark_subtree_recascade(root),
        }
    }

    fn change_style_rules(&mut self, change: impl FnOnce(&mut StyleEngine)) {
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

    #[test]
    #[should_panic(expected = "CSS rule belongs to another Document")]
    fn a_rule_from_another_document_is_refused() {
        let other = document();
        let rule = other
            .build_style_rule(".a", [declaration("width", "1px")])
            .expect("a style rule");
        document().append_rules(vec![rule]);
    }

    #[test]
    fn trees_mounting_one_parsed_sheet_share_its_rule_data() {
        let mut doc = crate::test_common::Doc::new();
        let root = doc.root;
        let sheet = doc
            .dom
            .parse_shadow_stylesheet("view { color: rgb(255, 0, 0) }");
        let mut trees = Vec::new();
        for _ in 0..2 {
            let host = doc.el(root, "view");
            let shadow = doc
                .dom
                .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
            doc.dom.add_shadow_sheet(shadow, &sheet);
            let inner = doc.el(shadow, "view");
            trees.push((shadow, inner));
        }
        doc.flush();

        let data = |shadow: NodeId| {
            doc.dom
                .get(shadow)
                .and_then(crate::tree::node::Node::shadow_data)
                .expect("a shadow root")
                .styles
                .data
                .clone()
        };
        assert!(Arc::ptr_eq(&data(trees[0].0), &data(trees[1].0)));
        for (_, inner) in trees {
            assert_eq!(doc.color(inner), crate::test_common::rgb(255, 0, 0));
        }
    }

    #[test]
    fn replacing_a_shared_sheet_on_one_tree_leaves_the_others_alone() {
        let mut doc = crate::test_common::Doc::new();
        let root = doc.root;
        let sheet = doc
            .dom
            .parse_shadow_stylesheet("view { color: rgb(0, 0, 255) }");
        let mut trees = Vec::new();
        for _ in 0..2 {
            let host = doc.el(root, "view");
            let shadow = doc
                .dom
                .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
            doc.dom.add_shadow_sheet(shadow, &sheet);
            let inner = doc.el(shadow, "view");
            trees.push((shadow, inner));
        }
        doc.flush();
        let data = |doc: &crate::test_common::Doc, shadow: NodeId| {
            doc.dom
                .get(shadow)
                .and_then(crate::tree::node::Node::shadow_data)
                .expect("a shadow root")
                .styles
                .data
                .clone()
        };
        let shared = data(&doc, trees[1].0);

        doc.dom
            .replace_shadow_stylesheet(trees[0].0, 0, "view { color: rgb(255, 0, 0) }");
        doc.flush();

        assert_eq!(doc.color(trees[0].1), crate::test_common::rgb(255, 0, 0));
        assert_eq!(doc.color(trees[1].1), crate::test_common::rgb(0, 0, 255));
        assert!(!Arc::ptr_eq(
            &data(&doc, trees[0].0),
            &data(&doc, trees[1].0)
        ));
        assert!(Arc::ptr_eq(&data(&doc, trees[1].0), &shared));
    }

    #[test]
    #[should_panic(expected = "ShadowSheet belongs to another Document")]
    fn a_shadow_sheet_from_another_document_is_refused() {
        let sheet = document().parse_shadow_stylesheet("view { width: 1px }");
        let mut document = document();
        let host = document.create_element("view", ());
        let root = document.document_element().id();
        document.append_child(root, host);
        let shadow = document.attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        document.add_shadow_sheet(shadow, &sheet);
    }
}
