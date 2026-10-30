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

/// An author stylesheet scoped to one shadow tree.
///
/// The document-level sheets are stylo's [`DocumentStyleSheet`], which
/// reports no implicit `@scope` root (css-cascade-6 §2.5: the root of an
/// `@scope` rule without a `<scope-start>`), so stylo falls back to the
/// document element, Gecko's root for a sheet with no owner node. A shadow
/// tree's sheet has no owner node either, but its containing node tree is
/// the shadow tree, so its implicit root is the shadow host — the root a
/// constructed sheet adopted by a shadow root resolves to in a browser.
/// [`ImplicitScopeRoot::Constructed`] is that root: stylo resolves it to the
/// host of the tree whose rules are being matched, and it may match the host
/// itself.
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

/// The lock every user-agent stylesheet is parsed and read under: one for
/// the process, as Gecko's is, rather than one per document like the lock an
/// engine's author sheets and inline styles take.
///
/// The one lock is what allows sharing. A sheet's rules are [`Locked`] to the
/// lock they were parsed under, and a stylist reads them with a guard of that
/// lock, so a sheet parsed under this one can be mounted by every document in
/// the process — the engine's UA sheet, parsed once per page configuration
/// instead of once per document ([`UserAgentStylesheet`]) — and stylo's
/// cascade-data cache for the user-agent origin, keyed by the sheet contents
/// mounted, then answers a new document with the cascade data an earlier one
/// built.
/// Stylo reads each origin's rules with the guard of its own lock
/// ([`StylesheetGuards`]`::ua_or_user` against `::author`), which is why a
/// second lock costs nothing in the style system itself.
///
/// Nothing writes under this lock: a user-agent sheet is never edited after it
/// is parsed. That matters because stylo's lock is a borrow counter, not a
/// mutex — a write while any thread holds a read is a panic — and documents on
/// different threads read under it at once.
static USER_AGENT_LOCK: LazyLock<SharedRwLock> = LazyLock::new(SharedRwLock::new);

/// A read of both locks a stylist's rules are under, held together: the
/// process-wide [`USER_AGENT_LOCK`] and one engine's own author lock. Every
/// stylist operation that reads rules of more than one origin — a flush, a
/// traversal, a media change — takes its [`StylesheetGuards`] from here.
pub(crate) struct RuleGuards<'a> {
    user_agent: SharedRwLockReadGuard<'static>,
    author: SharedRwLockReadGuard<'a>,
}

impl RuleGuards<'_> {
    /// Both guards, as stylo takes them.
    pub(crate) fn guards(&self) -> StylesheetGuards<'_> {
        StylesheetGuards {
            ua_or_user: &self.user_agent,
            author: &self.author,
        }
    }

    /// The guard that reads rules of `origin`.
    fn for_origin(&self, origin: Origin) -> &SharedRwLockReadGuard<'_> {
        match origin {
            Origin::UserAgent | Origin::User => &self.user_agent,
            Origin::Author => &self.author,
        }
    }
}

/// Reads [`USER_AGENT_LOCK`] and `author` together.
pub(crate) fn rule_guards(author: &SharedRwLock) -> RuleGuards<'_> {
    RuleGuards {
        user_agent: USER_AGENT_LOCK.read(),
        author: author.read(),
    }
}

/// A user-agent stylesheet parsed once, for any number of documents to mount.
///
/// Parsed under the process-wide user-agent lock, so every document reads it
/// with the guard it already takes for the user-agent origin, and mounting
/// it copies nothing: the documents share the one parsed sheet, and, through
/// stylo's user-agent cascade-data cache, the cascade data built from it.
/// This is how an embedder that opens many documents over one UA sheet —
/// one per page configuration — parses it once.
#[derive(Clone, Debug)]
pub struct UserAgentStylesheet(Arc<Stylesheet>);

impl UserAgentStylesheet {
    /// Parses `css` as a user-agent sheet. Its base URL is `about:blank`,
    /// as every document's own is.
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

/// Parses one stylesheet whose rules are locked under `lock`.
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

/// One parsed shadow-tree stylesheet, branded with the document that parsed
/// it, for mounting on any number of that document's shadow trees.
///
/// Every tree it is mounted on holds the same sheet, which is what lets
/// stylo build the trees' rule data once: its cascade-data cache is keyed on
/// sheet identity, so trees mounting the same sheets share one copy, where a
/// sheet parsed per tree builds one per tree. A component whose instances all
/// carry the same rules parses them once per document
/// ([`Document::parse_shadow_stylesheet`]) and mounts the result on each
/// instance ([`Document::add_shadow_sheet`]).
///
/// Like a [`CssRule`], it can only be mounted by the document that parsed it.
#[derive(Clone, Debug)]
pub struct ShadowSheet {
    inner: ShadowStyleSheet,
    owner: StdArc<SharedRwLock>,
}

/// One `setProperty` call, parsed and checked against the block it updates
/// ([`StyleEngine::parse_style_update`]): it is known to change that block,
/// and carries the positions in it the change lands on, so applying it
/// ([`StyleEngine::apply_style_update`]) does not search the block again.
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
    /// Applies the update to the block it was checked against, or to an
    /// unchanged copy of it.
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

/// What a change to the style context requires of the tree that was styled
/// under the old one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use]
pub(crate) enum StyleInvalidation {
    /// Which rules apply is unchanged; only their computed values can move.
    Recascade,
    /// The set of applicable rules may have moved, so selectors have to run
    /// again.
    Rematch,
}

/// One `@font-face` rule reduced to what a resource loader needs.
///
/// The `format()` and `tech()` hints on a `src` component are dropped: they
/// exist so a browser can skip a face it could not decode, and this engine
/// hands every candidate to the loader in author order and takes the first
/// that loads. The matching descriptors — `font-style`, `font-weight`,
/// `font-stretch`, `unicode-range` — are dropped because nothing reads them:
/// a loaded face is registered under the declared family carrying its own
/// metadata, see
/// [`TextContext::register_font_face`](seldon::text::TextContext::register_font_face).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFaceRequest {
    /// The `font-family` descriptor: the family the face is registered under.
    pub family: String,
    /// The `src` components in author order. The first one that loads wins.
    pub sources: Vec<FontFaceSource>,
}

/// One component of an `@font-face` rule's `src` descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFaceSource {
    /// `url(...)`, resolved against the document's base URL. An embedder
    /// fetches it; nothing in `terminus` performs IO.
    Url(String),
    /// `local(...)`: a face already installed on the platform, by name.
    Local(String),
}

/// Reduces one parsed `@font-face` rule to a loadable request.
///
/// `None` for a rule that names no family or offers no usable source — a
/// rule a browser would also never load anything for.
fn font_face_request(rule: &FontFaceRule) -> Option<FontFaceRequest> {
    let family = rule.descriptors.font_family.as_ref()?;
    let sources = rule
        .descriptors
        .src
        .as_ref()?
        .0
        .iter()
        .filter_map(|source| match source {
            // `as_str` is the URL resolved against the document's base; an
            // unresolvable one serializes as the empty string and is dropped
            // here rather than handed to a fetcher that could only fail.
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

/// The private stylo state owned by exactly one [`Document`].
pub(crate) struct StyleEngine {
    stylist: Stylist,
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
    /// Whether an author sheet has been mounted since the last time the
    /// `@font-face` rules were reported. Reading the rules takes the shared
    /// lock and walks every origin, and the answer can only change when a
    /// sheet arrives, so this keeps the ordinary "nothing new" call free.
    font_faces_dirty: bool,
    /// Every `@font-face` rule already reported, kept alive so that pointer
    /// identity stays unique: a rule survives the cascade rebuilds that
    /// re-collect it, and a strong reference is what stops a freed rule's
    /// address from being reused by a later one.
    reported_font_faces: Vec<Arc<Locked<FontFaceRule>>>,
    /// The `@position-try` names whose rules the stylist flushes since the
    /// last style harvest added, removed or changed
    /// (`CascadeDataDifference::changed_position_try_names`). No element's
    /// base style changes with them, so no restyle carries the news; the
    /// harvest re-cascades the position options that name one
    /// (css-anchor-position-1 §6.5.1, and `layout::anchors`). Empty for a
    /// page whose sheets declare no `@position-try` rule.
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

    /// Keeps what one stylist flush reports that the style harvest has to
    /// act on outside the element restyles it already produces.
    fn note_flush(pending: &mut FxHashSet<Atom>, invalidations: &StylesheetInvalidationSet) {
        let changed = &invalidations
            .cascade_data_difference
            .changed_position_try_names;
        pending.extend(changed.iter().cloned());
    }

    /// Takes the `@position-try` names changed since the last call.
    pub(crate) fn take_changed_position_try_names(&mut self) -> FxHashSet<Atom> {
        std::mem::take(&mut self.changed_position_try_names)
    }

    /// Every `@font-face` rule this engine has not reported before.
    ///
    /// The seam an embedder loads declared faces over: `terminus` never performs
    /// IO, so it names the rules and the caller fetches their sources and
    /// hands the bytes back through
    /// [`Document::register_font_face`](crate::tree::document::Document::register_font_face).
    ///
    /// A rule is reported once per document, whatever later sheets do to the
    /// cascade: the collected list is rebuilt from scratch on every stylist
    /// flush, and a face already handed to the font backend does not need
    /// loading again. Rules without a `font-family` or without a usable `src`
    /// are skipped, as is a `url()` that did not resolve against the
    /// document's base URL.
    ///
    /// Shadow-scoped sheets are not walked: their `@font-face` rules live in
    /// a scoped `AuthorStyles` rather than in the stylist's own origins, and
    /// no scoped sheet in this engine carries one yet.
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

    /// The engine's own lock: what its author sheets, its shadow trees'
    /// sheets and its inline styles are locked under. User-agent sheets are
    /// under [`USER_AGENT_LOCK`] instead; [`Self::rule_guards`] reads both.
    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    /// A read of both locks this engine's rules are under, for a stylist
    /// operation that reads rules of every origin.
    pub(crate) fn rule_guards(&self) -> RuleGuards<'_> {
        rule_guards(&self.lock)
    }

    pub(crate) fn supports_property(&self, property: &str) -> bool {
        PropertyId::parse(property, &self.parser_context(CssRuleType::Style)).is_ok()
    }

    /// Parses one CSSOM-style property update — `setProperty`'s semantics:
    /// an empty value removes the property — against the declaration block
    /// an element holds.
    ///
    /// Answers `None` for a property name or value that does not parse and
    /// for an update that would not change the block, so a caller can return
    /// before it records anything about the write.
    pub(crate) fn parse_style_update(
        &self,
        block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
        property: &str,
        value: &str,
    ) -> Option<PropertyUpdate> {
        let context = self.parser_context(CssRuleType::Style);
        let property = PropertyId::parse(property, &context).ok()?;
        if value.is_empty() {
            let first = block?
                .read_with(&self.lock.read())
                .first_declaration_to_remove(&property)?;
            return Some(PropertyUpdate::Remove { property, first });
        }
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

    /// Applies an update [`Self::parse_style_update`] checked against the
    /// block in `slot`, which must not have changed since. `slot` then holds
    /// the updated block, or `None` once it is empty.
    ///
    /// A block nothing else holds is updated where it is, keeping its
    /// identity. One the rule tree holds — the block the element's computed
    /// style was cascaded from — is copied and the copy is updated, as Gecko's
    /// `DeclarationBlock::EnsureMutable` copies a block the rule tree holds,
    /// for two reasons:
    ///
    /// - The flush finds a changed block by its identity. Replacing the style-attribute level of a
    ///   rule node, Stylo's `update_rule_at_level` (`vendor/stylo/style/rule_tree/mod.rs`) takes
    ///   the same `Arc` at that level for an unchanged block and keeps the old rule node, so a
    ///   block updated where the rule tree holds it would never be cascaded again. A
    ///   presentational-hint block is collected while matching, but it too is held by the rule node
    ///   it was matched into.
    /// - An animation tick before the next style flush cascades the element again from its rule
    ///   node, and has to see the declarations the element had at the last flush, which only the
    ///   flush replaces.
    ///
    /// Later writes before that flush find the copy unshared.
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

    /// Builds a complete inline declaration block from a record of
    /// name/value pairs.
    ///
    /// This is the batch counterpart of
    /// [`Self::parse_style_update`], for the setter whose semantics
    /// are a *replacement* rather than a mutation. Building from an empty
    /// block replaces the element's declarations in one pass, where the
    /// per-property path merges each declaration into the block the element
    /// already holds.
    ///
    /// Each declaration is parsed exactly as
    /// [`Self::parse_style_update`] parses one — a name that does
    /// not resolve, or a value that does not parse, drops that declaration
    /// and nothing else. An empty value is skipped: `setProperty` with an
    /// empty value removes a property, and there is nothing here to remove.
    ///
    /// Returns the block, `None` when nothing parsed. The `style` attribute
    /// reads back as its serialization, made when it is read
    /// ([`Node::attribute`](crate::tree::node::Node::attribute)).
    pub(crate) fn build_inline_style_block<'a>(
        &self,
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
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

    /// Parses one sheet under the lock its origin reads with: the
    /// process-wide user-agent lock for a user-agent sheet, this engine's
    /// own for an author sheet.
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

    /// Mounts a user-agent sheet parsed once for every document
    /// ([`UserAgentStylesheet`]), as the one `add_stylesheet` parses for this
    /// document alone would be.
    pub(crate) fn add_user_agent_stylesheet(&mut self, sheet: &UserAgentStylesheet) {
        self.mount_document_sheet(DocumentStyleSheet(Arc::clone(&sheet.0)), Origin::UserAgent);
    }

    /// Appends one document-level sheet of `origin` to the stylist and
    /// flushes it.
    fn mount_document_sheet(&mut self, sheet: DocumentStyleSheet, origin: Origin) {
        let guards = rule_guards(&self.lock);
        self.stylist
            .append_stylesheet(sheet, guards.for_origin(origin));
        let invalidations = self.stylist.flush(&guards.guards());
        drop(guards);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
        self.font_faces_dirty = true;
    }

    /// Parses an author sheet for shadow trees.
    fn parse_scoped_stylesheet(&self, css: &str) -> ShadowSheet {
        self.shadow_sheet(self.parse_stylesheet(css, Origin::Author))
    }

    /// [`Self::parse_scoped_stylesheet`] for a sheet the engine wrote itself:
    /// an author sheet of the tree, parsed with chrome rules enabled.
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

    /// Appends a parsed sheet to one shadow tree's sheets, as it is: every
    /// tree that mounts the same [`ShadowSheet`] shares its rule data.
    ///
    /// # Panics
    ///
    /// Panics if another engine parsed the sheet, or if the tree already
    /// mounts it.
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

    /// Replaces the scoped sheet at `index` with one parsed from `css`, in
    /// the same place in the tree's sheet order: the new sheet goes in
    /// before the old one, which then leaves.
    ///
    /// # Panics
    ///
    /// Panics if the tree has no sheet at `index`.
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
        // No device: a shadow tree's sheets collect no invalidations (stylo's
        // `AuthorStyles::flush` passes none either); the caller re-matches
        // the host's subtree instead.
        let data = styles.data.clone();
        styles
            .stylesheets
            .remove_stylesheet(None, data.custom_media_map(), old, &guard);
        let invalidations = styles.flush(&mut self.stylist, &guard);
        drop(guard);
        Self::note_flush(&mut self.changed_position_try_names, &invalidations);
    }

    /// Wraps rules this engine built into one author-origin sheet.
    ///
    /// # Panics
    ///
    /// Panics if any rule was built by a different engine.
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

    /// Installs rules this engine built as one author-origin sheet scoped to
    /// one shadow tree: built like [`Self::append_rules`], flushed like
    /// [`Self::add_scoped_sheet`].
    ///
    /// Like the text path, this leaves `font_faces_dirty` alone, so an
    /// `@font-face` rule in a shadow sheet is not loaded.
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

    /// Removes every sheet of one shadow tree, leaving it styled by no
    /// author rule of its own.
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

    /// Installs rules this engine built as one author-origin sheet.
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
        // `<keyframes-name>` is an ident *or* a string, and a producer that
        // preserves the authored prelude hands over the quotes with it. Parsing
        // resolves both spellings to the same atom, which is what
        // `animation-name` is matched against; taking the text as an ident
        // would leave `"spin"` unable to match `spin`, and would make the names
        // that only exist in string form (`"none"`) unreachable.
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

    /// Builds an `@font-face` rule from its descriptor block text.
    ///
    /// Font-face descriptors are not CSS properties, so stylo exposes no
    /// per-descriptor constructor and this one block is parsed as text. Its
    /// input is a single rule body, never a stylesheet.
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

    /// Re-evaluates the media queries against the device that was just
    /// changed, and reports what the tree now needs.
    ///
    /// Stylo answers which origins' rule sets a device change moved. An empty
    /// answer means every media query still evaluates the way it did, so no
    /// selector can start or stop matching and only the computed values that
    /// read the device — viewport units, `rpx`, resolution-dependent images,
    /// snapped border widths — have to be produced again.
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

    /// The viewport in CSS px.
    #[must_use]
    pub fn viewport_size(&self) -> euclid::default::Size2D<f32> {
        let size = self.device().viewport_size();
        euclid::default::Size2D::new(size.width, size.height)
    }

    /// CSS-px → device-px scale factor.
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

    /// Mounts a user-agent sheet parsed once for every document
    /// ([`UserAgentStylesheet::parse`]), where [`Self::add_stylesheet`] with
    /// [`Origin::UserAgent`] parses one for this document alone. Same
    /// cascade, same order rules: a later sheet cascades over an earlier
    /// one.
    pub fn add_user_agent_stylesheet(&mut self, sheet: &UserAgentStylesheet) {
        self.change_style_rules(|engine| engine.add_user_agent_stylesheet(sheet));
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
        self.change_style_rules(|engine| engine.append_rules(rules));
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
    ///
    /// This parses `css` for this one tree. Rules every instance of a
    /// component carries are parsed once instead
    /// ([`Self::parse_shadow_stylesheet`]) and mounted on each tree
    /// ([`Self::add_shadow_sheet`]); the two cascade alike.
    pub fn add_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_shadow_stylesheet: not a live shadow root");
        let sheet = self.style_engine().parse_scoped_stylesheet(css);
        self.mount_shadow_sheet(shadow_root, host, &sheet);
    }

    /// Parses an author stylesheet for this document's shadow trees, to be
    /// mounted on any number of them with [`Self::add_shadow_sheet`].
    ///
    /// The sheet cascades as one [`Self::add_shadow_stylesheet`] parses.
    #[must_use]
    pub fn parse_shadow_stylesheet(&self, css: &str) -> ShadowSheet {
        self.style_engine().parse_scoped_stylesheet(css)
    }

    /// [`Self::parse_shadow_stylesheet`] for a stylesheet the engine itself
    /// wrote: the sheet cascades as one
    /// [`Self::add_engine_shadow_stylesheet`] parses.
    #[must_use]
    pub fn parse_engine_shadow_stylesheet(&self, css: &str) -> ShadowSheet {
        self.style_engine().parse_engine_scoped_stylesheet(css)
    }

    /// Mounts a parsed stylesheet on one shadow tree, after the sheets the
    /// tree already has.
    ///
    /// The tree holds the sheet itself rather than a copy, so the trees one
    /// sheet is mounted on share the rule data stylo builds from it. It
    /// cascades exactly as the same text added with
    /// [`Self::add_shadow_stylesheet`] (or, for a sheet from
    /// [`Self::parse_engine_shadow_stylesheet`],
    /// [`Self::add_engine_shadow_stylesheet`]) would.
    ///
    /// Stylo reports the `@position-try` names a flush changed only when it
    /// builds rule data, which a tree reusing another's skips. Nothing misses
    /// them: the names reach only the tree and its host, and mounting restyles
    /// the host's whole subtree, whose restyled elements re-cascade their
    /// options whatever the report says.
    ///
    /// # Panics
    ///
    /// Panics if `shadow_root` is not a live shadow root, if another document
    /// parsed the sheet, or if the tree already mounts it.
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

    /// Adds a stylesheet the engine itself wrote to one shadow tree, such as
    /// a component's own rules for the HTML content it builds there.
    ///
    /// It cascades as [`Self::add_shadow_stylesheet`]'s sheets do, in the
    /// order sheets are added, but it parses with chrome rules enabled, so it
    /// can write the values the `lynx` grammar keeps from authors:
    /// `display: block`, `flow` and `inline`, the flow layout of that content.
    pub fn add_engine_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_engine_shadow_stylesheet: not a live shadow root");
        let sheet = self.style_engine().parse_engine_scoped_stylesheet(css);
        self.mount_shadow_sheet(shadow_root, host, &sheet);
    }

    /// Replaces the stylesheet at `index` among those
    /// [`Self::add_shadow_stylesheet`] and [`Self::add_engine_shadow_stylesheet`]
    /// added to one shadow tree, counting from the first added, with an author
    /// sheet, keeping its place in their order: a sheet a component
    /// rewrites, such as the one `<x-markdown>` builds from its
    /// `markdown-style` attribute, still cascades after the sheets added
    /// before it and before the ones added after it, as replacing the text of
    /// a `<style>` element in a shadow tree keeps that element's place.
    ///
    /// # Panics
    ///
    /// Panics if `shadow_root` is no live shadow root, or has no sheet at
    /// `index`.
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

    /// Appends rules this document built as one author-origin stylesheet
    /// scoped to one shadow tree.
    ///
    /// This is [`Document::append_rules`] for a shadow root, and the
    /// pre-parsed counterpart of [`Document::add_shadow_stylesheet`]: the
    /// sheet's rules match only inside `shadow_root`'s tree (and the host,
    /// through `:host`), and its implicit `@scope` root is the host. Later
    /// calls cascade over earlier sheets of the same shadow root.
    ///
    /// An `@font-face` rule in the sheet is not loaded: font faces are
    /// collected from the document's sheets only, as for a constructed sheet
    /// adopted by a shadow root in Chromium (`adoptedStyleSheets`). The text
    /// path, [`Document::add_shadow_stylesheet`], behaves the same way.
    ///
    /// # Panics
    ///
    /// Panics if `shadow_root` is not a live shadow root, or if any rule was
    /// built by a different document.
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

    /// Removes every sheet [`Document::add_shadow_stylesheet`] and
    /// [`Document::append_shadow_rules`] mounted on one shadow tree: the
    /// shadow-root counterpart of emptying a shadow root's
    /// `adoptedStyleSheets` (CSSOM §Using CSS style sheets). The tree is
    /// matched again against the document's UA rules alone.
    ///
    /// # Panics
    ///
    /// Panics if `shadow_root` is not a live shadow root.
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

    /// A change to the device the document is styled against. The engine says
    /// whether any media query changed its answer; only then does the tree
    /// need its selectors run again.
    fn change_device(&mut self, change: impl FnOnce(&mut StyleEngine) -> StyleInvalidation) {
        self.note_visual_mutation();
        let invalidation = change(self.style_engine_mut());
        let root = self.document_element().id();
        match invalidation {
            StyleInvalidation::Rematch => self.mark_subtree_dirty(root),
            StyleInvalidation::Recascade => self.mark_subtree_recascade(root),
        }
    }

    /// A change to the rule set itself, which always re-matches: a rule that
    /// did not exist before cannot be known not to match.
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

    /// Every tree one parsed sheet is mounted on holds that sheet, so stylo
    /// builds their rule data once and the trees share it; the rules match in
    /// each tree as the text path's do.
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

    /// Replacing a shared sheet on one tree leaves every other tree mounting
    /// it with the shared sheet and the shared rule data.
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
