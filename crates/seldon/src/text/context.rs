//! Reusable Parley font and layout contexts.

use core::fmt;
use core::ops::Range;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use icu_properties::props::{
    DefaultIgnorableCodePoint, EmojiPresentation, GeneralCategory, Script as UnicodeScript,
    WhiteSpace,
};
use icu_properties::script::ScriptWithExtensions;
use icu_properties::{CodePointMapData, CodePointSetData, PropertyNamesShort};
use parley::fontique::{
    Attributes, Collection, CollectionOptions, FallbackKey, FamilyId, FontInfoOverride,
    GenericFamily, Language, QueryFamily, QueryFont, QueryStatus, Script, ScriptExt, SourceCache,
};
use parley::{FontContext, FontFamily, FontFamilyName, Layout, LayoutContext};

use super::FontBlob;
use crate::style::TextBrush;

fn system_font_template() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();

    TEMPLATE.get_or_init(FontContext::new)
}

/// The script fontique adds the language-less fallback list of to every
/// segment's query, whatever the segment's script (fontique 0.12
/// `collection/query.rs:105-111`, its workaround for parley#597): the list a
/// Common-script character with no script of its own, `★` or a CJK
/// punctuation mark in a Latin run, is found in. Not a choice made here; it
/// is what fontique would consult for any character the run's own families
/// lack, so a missing character asks for it too ([`TextContext::cover`]).
const FONTIQUE_EVERY_SEGMENT_SCRIPT: Script = Script::from_bytes(*b"Hani");

/// A script and the language its fallback lookup names, `None` for a lookup
/// that names none: fontique's normalized key, without the bookkeeping its
/// `FallbackKey` adds, so that `zh-Hant` and `zh-TW` are one entry and a
/// language fontique does not distinguish for the script is no language.
type PlatformKey = (Script, Option<Language>);

/// The platform's fallback for every (script, language) any context in the
/// process asked for: one table for the process.
///
/// Fontique keeps what the platform answers in the context that asked, and
/// every context clones the template, so without this each one asks again —
/// and CoreText keeps no answer to a lookup that names a language, so each
/// such ask is paid again too. The families are the template's system
/// families, the same in every context cloned from it.
fn platform_fallbacks() -> &'static Mutex<HashMap<PlatformKey, Arc<[FamilyId]>>> {
    static FALLBACKS: OnceLock<Mutex<HashMap<PlatformKey, Arc<[FamilyId]>>>> = OnceLock::new();

    FALLBACKS.get_or_init(Mutex::default)
}

fn platform_key(script: Script, locale: &Language) -> PlatformKey {
    (script, FallbackKey::new(script, Some(locale)).locale())
}

/// The platform's fallback for `script` in `locale`'s language, asked of the
/// platform the first time any context in the process needs it and read
/// from [`platform_fallbacks`] every time after.
fn platform_fallback(script: Script, locale: &Language) -> Arc<[FamilyId]> {
    let key = platform_key(script, locale);
    let known = platform_fallbacks()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(families) = known {
        return families;
    }
    // Asked without the lock held, of a clone of the template that has no
    // key filled: two contexts that miss at once both ask, and both answers
    // are the same.
    let families: Arc<[FamilyId]> = system_font_template()
        .clone()
        .collection
        .fallback_families(FallbackKey::new(script, key.1.as_ref()))
        .collect();
    platform_fallbacks()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert(families)
        .clone()
}

/// The script fontique keeps a fallback list for that `unicode` names, if
/// any: none for Common, Inherited and Unknown.
fn fontique_script(unicode: UnicodeScript) -> Option<Script> {
    if matches!(
        unicode,
        UnicodeScript::Common | UnicodeScript::Inherited | UnicodeScript::Unknown
    ) {
        return None;
    }
    let code = PropertyNamesShort::<UnicodeScript>::new().get(unicode)?;
    let bytes = <[u8; 4]>::try_from(code.as_bytes()).ok()?;
    Some(Script::from_bytes(bytes))
}

/// The scripts of `text`'s characters that have one of their own, in the
/// order they first appear: the scripts parley can itemize its segments in.
fn scripts_in(text: &str) -> Vec<Script> {
    let map = CodePointMapData::<UnicodeScript>::new();
    let mut scripts = Vec::new();
    for script in text.chars().filter_map(|ch| fontique_script(map.get(ch))) {
        if !scripts.contains(&script) {
            scripts.push(script);
        }
    }
    scripts
}

/// The scripts whose fallback list fontique could find `ch` in, inside a
/// span whose characters with a script of their own are in `span_scripts`.
///
/// A character with a script of its own is in a segment of that script. A
/// Common or Inherited one takes the script of the characters around it, so
/// its segment's script is one of its `Script_Extensions` that the span also
/// holds; fontique adds [`FONTIQUE_EVERY_SEGMENT_SCRIPT`]'s language-less list
/// to every segment besides, which the caller adds.
fn scripts_of(ch: char, span_scripts: &[Script], scripts: &mut Vec<Script>) {
    let push = |script: Script, scripts: &mut Vec<Script>| {
        if !scripts.contains(&script) {
            scripts.push(script);
        }
    };
    if let Some(script) = fontique_script(CodePointMapData::<UnicodeScript>::new().get(ch)) {
        push(script, scripts);
        return;
    }
    for unicode in ScriptWithExtensions::new()
        .get_script_extensions_val(ch)
        .iter()
    {
        if let Some(script) = fontique_script(unicode)
            && span_scripts.contains(&script)
        {
            push(script, scripts);
        }
    }
}

/// The families a `font-family` value names in `collection`, in order, as
/// parley resolves them: a name to its family, a generic to the families it
/// maps to.
fn resolve_families(
    collection: &mut Collection,
    family: &FontFamily<'_>,
    families: &mut Vec<FamilyId>,
) {
    let mut push = |name: &FontFamilyName<'_>| match name {
        FontFamilyName::Named(name) => families.extend(collection.family_id(name)),
        FontFamilyName::Generic(generic) => {
            families.extend(collection.generic_families(*generic));
        }
    };
    match family {
        FontFamily::Source(source) => {
            for name in FontFamilyName::parse_css_list(source).map_while(Result::ok) {
                push(&name);
            }
        }
        FontFamily::Single(name) => push(name),
        FontFamily::List(names) => names.iter().for_each(push),
    }
}

/// Whether `ch` needs no glyph of a font for the coverage check: white
/// space, controls and default-ignorable code points, which no font is asked
/// to draw, and emoji-presentation characters, which parley sets in the
/// emoji family whatever the run's own families are.
fn needs_no_glyph(ch: char) -> bool {
    CodePointSetData::new::<WhiteSpace>().contains(ch)
        || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(ch)
        || CodePointSetData::new::<EmojiPresentation>().contains(ch)
        || CodePointMapData::<GeneralCategory>::new().get(ch) == GeneralCategory::Control
}

/// The script an ISO 15924 code names, when fontique has sample text to ask
/// the platform for its fallback with.
fn parse_script(code: &str) -> Option<Script> {
    let bytes: [u8; 4] = code.as_bytes().try_into().ok()?;
    let script = Script::from_bytes(bytes);
    script.sample().is_some().then_some(script)
}

/// The largest text, in UTF-8 bytes, whose shaping buffers the scratch
/// layout keeps for the next shaping: parley reserves about this many
/// characters, clusters and glyphs, so it bounds what one context holds
/// between shapings at about a megabyte. Ordinary paragraphs are far
/// smaller; one past it frees the buffers once its copy is taken.
const SCRATCH_KEEP_BYTES: usize = 16 * 1024;

/// Reusable resources for text shaping and layout.
pub struct TextContext {
    font: FontContext,
    layout: LayoutContext<TextBrush>,
    /// The layout every shaping builds into first.
    ///
    /// Parley sizes a fresh layout's shaping buffers for the text's UTF-8
    /// byte length, an upper bound on its characters, and never shrinks them:
    /// it is built for a caller that reuses one layout. Shaping here, and
    /// keeping a copy sized to what was shaped
    /// (`super::paragraph::shape::shape`), keeps that bound out of every
    /// retained paragraph. Measurement probes read it in place and keep
    /// nothing. Its own buffers serve the next shaping, and a paragraph
    /// larger than [`SCRATCH_KEEP_BYTES`] does not leave them behind.
    scratch: Layout<TextBrush>,
    /// The language of the text this context shapes, which every shaping
    /// hands parley as the run's locale; see [`Self::set_locale`].
    locale: Option<Language>,
    /// The families [`Self::register_fallback_fonts`] appended to every
    /// script's fallback list, kept so that a locale set later can file them
    /// under its own keys too.
    fallback_families: Vec<FamilyId>,
    /// Whether this context was cloned from the system-font template, and so
    /// asks the platform for what no registered family covers; see
    /// [`platform_fallbacks`].
    system_fonts: bool,
    /// Whether the platform's fallback is asked only for a character the
    /// run's own families lack ([`Self::cover`]): set by
    /// [`Self::set_locale`] for a context the platform answers, one with
    /// system fonts and no fallback container of its own.
    fallback_on_miss: bool,
    /// The scripts whose fallback lists in this context already hold the
    /// platform's answer rather than the empty list
    /// [`Self::set_locale`] files.
    answered_scripts: Vec<Script>,
    #[cfg(test)]
    shape_count: usize,
}

impl TextContext {
    #[must_use]
    pub fn new() -> Self {
        // Discovering the system fonts calls the platform's font API
        // (CoreText, fontconfig, DirectWrite), which Miri cannot execute; a
        // context under Miri has only the fonts registered into it.
        if cfg!(miri) {
            return Self::without_system_fonts();
        }
        Self {
            font: system_font_template().clone(),
            layout: LayoutContext::new(),
            scratch: Layout::default(),
            locale: None,
            fallback_families: Vec::new(),
            system_fonts: true,
            fallback_on_miss: false,
            answered_scripts: Vec::new(),
            #[cfg(test)]
            shape_count: 0,
        }
    }

    /// Discovers the platform's fonts before the first context that needs
    /// them.
    ///
    /// Every [`Self::new`] clones one process-wide template, and the first
    /// call builds it: fontique enumerates the platform's font files and
    /// reads each one's name table, tens to hundreds of milliseconds on a
    /// desktop. Without this call that work runs inside whichever call first
    /// needs a context, on that call's thread. This call runs it here
    /// instead, on the calling thread, and returns once the template exists.
    ///
    /// Idempotent, from any thread: a call after the template exists returns
    /// at once, and one that overlaps the building call waits for it. What
    /// it does not cover: a context's own registered fonts, and the
    /// platform's per-script fallback, which [`Self::preload_fallbacks`]
    /// asks for the scripts an embedder names.
    pub fn preload_system_fonts() {
        let _ = system_font_template();
    }

    /// Asks the platform for the fallback of each of `scripts`, ISO 15924
    /// codes such as `Hani` or `Arab`, in `locale`'s language, ahead of the
    /// first context that would, into the process's one table.
    ///
    /// A context asks only when a character is missing from its run's own
    /// families ([`Self::cover`]), and the first context in the process to
    /// ask for a (script, language) pays the platform for it: on a Mac up to
    /// a few milliseconds, about 8 ms for the first lookup in the process
    /// that names no language, which has CoreText build its whole default
    /// cascade list. This pays it here instead, for the scripts the embedder
    /// names. Builds the template first, as [`Self::preload_system_fonts`]
    /// does; a script already answered for the language is not asked again.
    ///
    /// Returns `false`, having done nothing, when `locale` is not a
    /// well-formed BCP-47 language tag or a script is not one
    /// [`Self::is_script`] accepts.
    #[must_use]
    pub fn preload_fallbacks(locale: &str, scripts: &[&str]) -> bool {
        let Ok(locale) = Language::parse(locale) else {
            return false;
        };
        let Some(scripts) = scripts
            .iter()
            .map(|code| parse_script(code))
            .collect::<Option<Vec<Script>>>()
        else {
            return false;
        };
        for script in scripts {
            let _ = platform_fallback(script, &locale);
        }
        true
    }

    /// Whether `tag` is a tag [`Self::set_locale`] and
    /// [`Self::preload_fallbacks`] accept: a well-formed BCP-47 language tag.
    #[must_use]
    pub fn is_locale(tag: &str) -> bool {
        Language::parse(tag).is_ok()
    }

    /// Whether `code` is a script [`Self::preload_fallbacks`] accepts: an
    /// ISO 15924 code, spelled as the standard spells it, such as `Hani`,
    /// that fontique has sample text to ask the platform with.
    #[must_use]
    pub fn is_script(code: &str) -> bool {
        parse_script(code).is_some()
    }

    #[must_use]
    pub fn without_system_fonts() -> Self {
        Self {
            font: FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: false,
                }),
                source_cache: SourceCache::default(),
            },
            layout: LayoutContext::new(),
            scratch: Layout::default(),
            locale: None,
            fallback_families: Vec::new(),
            system_fonts: false,
            fallback_on_miss: false,
            answered_scripts: Vec::new(),
            #[cfg(test)]
            shape_count: 0,
        }
    }

    /// Registers an owned font resource without copying its byte payload.
    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        self.font
            .collection
            .register_fonts(data.into_inner(), None)
            .into_iter()
            .map(|(_, fonts)| fonts.len())
            .sum()
    }

    /// Registers an owned font resource under a CSS-declared family name.
    ///
    /// What an `@font-face` rule's `src` resolves to: the face is filed under
    /// the rule's `font-family` rather than under the name inside the font
    /// file, so a run naming the declared family shapes with it. The other
    /// overridable metadata — width, style, weight, variation axes — stays the
    /// font's own, because this engine reads none of the matching descriptors
    /// beside `font-family` and `src` yet.
    ///
    /// Returns how many faces the blob contributed, like
    /// [`Self::register_fonts`].
    pub fn register_font_face(&mut self, family: &str, data: FontBlob) -> usize {
        self.font
            .collection
            .register_fonts(
                data.into_inner(),
                Some(FontInfoOverride {
                    family_name: Some(family),
                    ..FontInfoOverride::default()
                }),
            )
            .into_iter()
            .map(|(_, fonts)| fonts.len())
            .sum()
    }

    /// Selects a registered family as the embedder-provided platform default.
    ///
    /// Native contexts already discover the platform's generic-family maps.
    /// Embedders without a system-font backend, notably Wasm, can use this
    /// after [`Self::register_fonts`] to give CSS's `system-ui`, `sans-serif`,
    /// and `serif` families a concrete first choice. Existing platform
    /// fallbacks remain available after the selected family.
    ///
    /// Returns `false` without changing the maps when `family` is unknown.
    pub fn set_default_font_family(&mut self, family: &str) -> bool {
        let Some(family) = self.font.collection.family_id(family) else {
            return false;
        };
        for generic in [
            GenericFamily::SystemUi,
            GenericFamily::SansSerif,
            GenericFamily::Serif,
        ] {
            self.font
                .collection
                .set_generic_families(generic, std::iter::once(family));
        }
        true
    }

    /// Registers an owned font resource and appends each family it holds to
    /// every script's fallback list.
    ///
    /// A fallback list is what shapes a cluster no family of the run's own
    /// `font-family` covers. Native contexts ask the platform for one per
    /// script; an embedder without a system-font backend, notably Wasm, has
    /// none, so text in a script its default family lacks shapes as missing
    /// glyphs whatever the page's CSS names. The families are offered for
    /// every script rather than for the ones they are known to cover, because
    /// selection already checks each candidate's character map per cluster.
    /// They are taken from the container rather than named by the embedder,
    /// which may hold a platform font whose family name it cannot predict.
    ///
    /// A script given a list here no longer asks the platform for its own, so
    /// an embedder that has a system-font backend should leave this alone.
    ///
    /// Returns how many faces the blob contributed, like
    /// [`Self::register_fonts`].
    pub fn register_fallback_fonts(&mut self, data: FontBlob) -> usize {
        let registered = self.font.collection.register_fonts(data.into_inner(), None);
        let families: Vec<FamilyId> = registered.iter().map(|(family, _)| *family).collect();
        for &(script, _) in Script::all_samples() {
            self.font
                .collection
                .append_fallbacks(script, families.iter().copied());
        }
        if let Some(locale) = self.locale {
            self.append_locale_fallbacks(&families, &locale);
        }
        self.fallback_families.extend(families);
        registered.iter().map(|(_, fonts)| fonts.len()).sum()
    }

    /// Appends `families` under `locale`'s key for every script fontique
    /// keeps a separate list for that script and language (Han for `ja`,
    /// `ko` and the `zh` regions, among others), so that a run shaped with
    /// the locale reaches them as a run shaped without one does. Like the
    /// language-less keys, a key given a list this way no longer asks the
    /// platform for its own.
    fn append_locale_fallbacks(&mut self, families: &[FamilyId], locale: &Language) {
        for &(script, _) in Script::all_samples() {
            let key = FallbackKey::new(script, Some(locale));
            if !key.is_default() {
                self.font
                    .collection
                    .append_fallbacks(key, families.iter().copied());
            }
        }
    }

    /// Sets the language of the text this context shapes, as a BCP-47 tag
    /// such as `zh-Hant`, `ja` or `en-US`.
    ///
    /// Every shaping hands it to parley as the run's locale, and parley
    /// passes it on to fontique's fallback key, so the platform's per-script
    /// fallback is asked with it: on Apple platforms CoreText then answers a
    /// Han cluster with `PingFang TC` for `zh-Hant`, `PingFang HK` for `zh-HK`,
    /// Hiragino Sans for `ja` and Apple SD Gothic Neo for `ko`, where without
    /// a language it answers `PingFang SC` for all of them. What Lynx iOS does
    /// with its locale-chosen cascade family, here for every script fontique
    /// distinguishes by language.
    ///
    /// The platform is asked for nothing here, and the fallback lists of
    /// every script are filed empty, under the language's keys and the
    /// language-less ones alike: fontique, which looks a segment's list up
    /// at the start of every segment whether a character in it is missing
    /// or not, then finds one and asks the platform for nothing. A script's
    /// list is filled with the platform's answer only when a shaping finds
    /// a character of it that the run's own families lack ([`Self::cover`]),
    /// from the process's one table, which asks the platform the first time
    /// any context in the process needs it. The answer is filed under the
    /// script's language-less key too, which is how, on a Traditional
    /// Chinese page, Common-script punctuation in a run with no Han
    /// character is set in the Traditional face: fontique appends the
    /// language-less Han list to every query.
    ///
    /// Families already registered through
    /// [`Self::register_fallback_fonts`] are filed under the language's keys
    /// instead, and answer for this context alone, which then asks the
    /// platform for nothing, as under the language-less keys.
    ///
    /// Returns `false`, without changing anything, when `tag` is not a
    /// well-formed BCP-47 language tag.
    pub fn set_locale(&mut self, tag: &str) -> bool {
        let Ok(locale) = Language::parse(tag) else {
            return false;
        };
        self.locale = Some(locale);
        let fallback_families = std::mem::take(&mut self.fallback_families);
        self.append_locale_fallbacks(&fallback_families, &locale);
        self.fallback_families = fallback_families;
        self.answered_scripts.clear();
        self.fallback_on_miss = self.system_fonts && self.fallback_families.is_empty();
        if self.fallback_on_miss {
            for &(script, _) in Script::all_samples() {
                let key = FallbackKey::new(script, Some(&locale));
                if key.locale().is_some() {
                    self.font.collection.set_fallbacks(key, core::iter::empty());
                }
                self.font
                    .collection
                    .set_fallbacks(script, core::iter::empty());
            }
        }
        true
    }

    /// Fills, before a shaping, the fallback list of every script a
    /// character of `text` is written in that its span's own families lack,
    /// with the platform's answer from the process's one table
    /// ([`platform_fallback`]); nothing for a context whose locale did not
    /// set it up ([`Self::fallback_on_miss`]).
    ///
    /// `spans` are each span's byte range in `text`, its `font-family` and
    /// its font attributes, as parley will shape it. A character is missing
    /// when no face that family list matches at those attributes maps it to
    /// a glyph, the test parley's own font selection makes; white space,
    /// controls, default-ignorable code points and emoji-presentation
    /// characters are not tested ([`needs_no_glyph`]). The lists filled for
    /// a missing character are the ones fontique would look it up in: its
    /// segment's script ([`scripts_of`]) and the script fontique adds to
    /// every query ([`FONTIQUE_EVERY_SEGMENT_SCRIPT`]).
    pub(in crate::text) fn cover<'span, 'family: 'span>(
        &mut self,
        text: &str,
        spans: impl IntoIterator<Item = (Range<usize>, &'span FontFamily<'family>, Attributes)>,
    ) {
        if !self.fallback_on_miss {
            return;
        }
        let mut wanted: Vec<Script> = Vec::new();
        let mut families: Vec<FamilyId> = Vec::new();
        let mut fonts: Vec<QueryFont> = Vec::new();
        for (bytes, family, attributes) in spans {
            let Some(span) = text.get(bytes) else {
                continue;
            };
            if span.chars().all(needs_no_glyph) {
                continue;
            }
            families.clear();
            resolve_families(&mut self.font.collection, family, &mut families);
            fonts.clear();
            let mut query = self.font.collection.query(&mut self.font.source_cache);
            query.set_families(families.iter().copied().map(QueryFamily::Id));
            query.set_attributes(attributes);
            query.matches_with(|font| {
                fonts.push(font.clone());
                QueryStatus::Continue
            });
            drop(query);
            let charmaps: Vec<_> = fonts.iter().filter_map(QueryFont::charmap).collect();
            let mut span_scripts = None;
            for ch in span.chars() {
                if needs_no_glyph(ch)
                    || charmaps
                        .iter()
                        .any(|charmap| charmap.map(ch).is_some_and(|glyph| glyph != 0))
                {
                    continue;
                }
                let span_scripts = span_scripts.get_or_insert_with(|| scripts_in(span));
                scripts_of(ch, span_scripts, &mut wanted);
                if !wanted.contains(&FONTIQUE_EVERY_SEGMENT_SCRIPT) {
                    wanted.push(FONTIQUE_EVERY_SEGMENT_SCRIPT);
                }
            }
        }
        let Some(locale) = self.locale else {
            return;
        };
        for script in wanted {
            if self.answered_scripts.contains(&script) {
                continue;
            }
            let answer = platform_fallback(script, &locale);
            let key = FallbackKey::new(script, Some(&locale));
            if key.locale().is_some() {
                self.font
                    .collection
                    .set_fallbacks(key, answer.iter().copied());
            }
            self.font
                .collection
                .set_fallbacks(script, answer.iter().copied());
            self.answered_scripts.push(script);
        }
    }

    /// The language [`Self::set_locale`] set, as parley takes it.
    #[must_use]
    pub fn locale(&self) -> Option<Language> {
        self.locale
    }

    /// The parley contexts and the scratch layout a shaping builds into.
    pub(super) fn shaping_parts(
        &mut self,
    ) -> (
        &mut FontContext,
        &mut LayoutContext<TextBrush>,
        &mut Layout<TextBrush>,
    ) {
        (&mut self.font, &mut self.layout, &mut self.scratch)
    }

    /// Releases the scratch layout's buffers when the shaping just done was
    /// of more than [`SCRATCH_KEEP_BYTES`]: one long paragraph should not
    /// keep its buffers for the life of the context.
    pub(super) fn trim_scratch(&mut self, text_len: usize) {
        if text_len > SCRATCH_KEEP_BYTES {
            self.scratch = Layout::default();
        }
    }

    #[cfg(test)]
    pub(super) fn record_shape(&mut self) {
        self.shape_count += 1;
    }

    #[cfg(test)]
    pub(super) const fn shape_count(&self) -> usize {
        self.shape_count
    }
}

impl Default for TextContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TextContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextContext")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const AHEM: &[u8] = include_bytes!("../../tests/fixtures/Ahem.ttf");

    #[test]
    fn deterministic_context_registers_embedded_fonts() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.font.collection.family_names().count(), 0);
        assert_eq!(
            context.register_fonts(FontBlob::from_static(b"not a font")),
            0
        );
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        assert!(context.font.collection.family_id("Ahem").is_some());
    }

    #[test]
    fn a_declared_family_files_the_face_under_the_declared_name() {
        let mut context = TextContext::without_system_fonts();

        assert_eq!(
            context.register_font_face("DeclaredAhem", FontBlob::from_static(AHEM)),
            1
        );

        assert!(context.font.collection.family_id("DeclaredAhem").is_some());
        assert!(
            context.font.collection.family_id("Ahem").is_none(),
            "the declared family replaces the name inside the font file"
        );
    }

    #[test]
    fn registration_retains_the_original_shared_blob() {
        let mut context = TextContext::without_system_fonts();
        let data = FontBlob::from_static(AHEM);
        let original_id = data.id();

        assert_eq!(context.register_fonts(data), 1);

        let family = context
            .font
            .collection
            .family_by_name("Ahem")
            .expect("the registered family is available");
        let font = family.default_font().expect("Ahem contains one face");
        let parley::fontique::SourceKind::Memory(retained) = font.source().kind() else {
            panic!("an in-memory font must retain an in-memory source");
        };
        assert_eq!(retained.id(), original_id);
    }

    #[test]
    fn embedder_default_populates_the_css_default_generic_families() {
        let mut context = TextContext::without_system_fonts();
        assert!(!context.set_default_font_family("Ahem"));
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        let ahem = context
            .font
            .collection
            .family_id("Ahem")
            .expect("registered family");

        assert!(context.set_default_font_family("Ahem"));

        for generic in [
            GenericFamily::SystemUi,
            GenericFamily::SansSerif,
            GenericFamily::Serif,
        ] {
            assert_eq!(
                context
                    .font
                    .collection
                    .generic_families(generic)
                    .collect::<Vec<_>>(),
                vec![ahem]
            );
        }
        assert!(
            context
                .font
                .collection
                .generic_families(GenericFamily::Monospace)
                .next()
                .is_none()
        );
    }

    #[test]
    fn a_fallback_container_serves_every_script() {
        let mut context = TextContext::without_system_fonts();
        let han = Script::from_bytes(*b"Hani");
        assert_eq!(
            context.register_fallback_fonts(FontBlob::from_static(b"not a font")),
            0
        );
        assert!(
            context
                .font
                .collection
                .fallback_families(han)
                .next()
                .is_none()
        );

        assert_eq!(
            context.register_fallback_fonts(FontBlob::from_static(AHEM)),
            1
        );

        let ahem = context
            .font
            .collection
            .family_id("Ahem")
            .expect("registered family");
        for script in [han, Script::from_bytes(*b"Latn")] {
            assert_eq!(
                context
                    .font
                    .collection
                    .fallback_families(script)
                    .collect::<Vec<_>>(),
                vec![ahem]
            );
        }
        assert!(
            context
                .font
                .collection
                .generic_families(GenericFamily::SansSerif)
                .next()
                .is_none(),
            "a fallback family is not a generic family"
        );
    }

    #[test]
    fn a_locale_must_be_a_language_tag() {
        let mut context = TextContext::without_system_fonts();
        assert!(context.locale().is_none());
        assert!(!context.set_locale("not a tag"));
        assert!(context.locale().is_none());
        assert!(context.set_locale("zh-Hant"));
        assert_eq!(
            context.locale().map(|locale| locale.as_str().to_owned()),
            Some("zh-Hant".to_owned())
        );
    }

    /// Fontique keeps a separate Han fallback list per language, so a
    /// fallback container registered under the language-less keys has to be
    /// filed under the language's too, whichever of the two is set first.
    #[test]
    fn fallback_containers_serve_the_locale_in_either_order() {
        let han = Script::from_bytes(*b"Hani");
        let japanese = Language::parse("ja").expect("a tag");
        let key = || FallbackKey::new(han, Some(&japanese));
        assert!(
            !key().is_default(),
            "the test needs a language fontique distinguishes"
        );

        let mut fonts_first = TextContext::without_system_fonts();
        assert_eq!(
            fonts_first.register_fallback_fonts(FontBlob::from_static(AHEM)),
            1
        );
        assert!(fonts_first.set_locale("ja"));
        let ahem = fonts_first
            .font
            .collection
            .family_id("Ahem")
            .expect("registered");
        assert_eq!(
            fonts_first
                .font
                .collection
                .fallback_families(key())
                .collect::<Vec<_>>(),
            vec![ahem]
        );
        assert_eq!(
            fonts_first
                .font
                .collection
                .fallback_families(han)
                .collect::<Vec<_>>(),
            vec![ahem]
        );

        let mut locale_first = TextContext::without_system_fonts();
        assert!(locale_first.set_locale("ja"));
        assert_eq!(
            locale_first.register_fallback_fonts(FontBlob::from_static(AHEM)),
            1
        );
        let ahem = locale_first
            .font
            .collection
            .family_id("Ahem")
            .expect("registered");
        assert_eq!(
            locale_first
                .font
                .collection
                .fallback_families(key())
                .collect::<Vec<_>>(),
            vec![ahem]
        );
        assert_eq!(
            locale_first
                .font
                .collection
                .fallback_families(han)
                .collect::<Vec<_>>(),
            vec![ahem]
        );
    }

    fn known_fallback(script: &str, tag: &str) -> Option<Vec<FamilyId>> {
        let language = Language::parse(tag).expect("a tag");
        let script = parse_script(script).expect("a script");
        platform_fallbacks()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&platform_key(script, &language))
            .map(|families| families.to_vec())
    }

    fn fallback_of(context: &mut TextContext, script: &str, tag: Option<&str>) -> Vec<FamilyId> {
        let language = tag.map(|tag| Language::parse(tag).expect("a tag"));
        let script = parse_script(script).expect("a script");
        context
            .font
            .collection
            .fallback_families(FallbackKey::new(script, language.as_ref()))
            .collect()
    }

    /// A system context that knows Ahem, the family every coverage test
    /// shapes in: it maps ASCII and nothing a fallback is asked for.
    fn system_context_with_ahem(tag: &str) -> TextContext {
        let mut context = TextContext::new();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        assert!(context.set_locale(tag));
        context
    }

    fn cover_in_ahem(context: &mut TextContext, text: &str) {
        let ahem = FontFamily::Single(FontFamilyName::Named("Ahem".into()));
        context.cover(text, [(0..text.len(), &ahem, Attributes::default())]);
    }

    #[test]
    fn a_preload_names_a_language_and_scripts() {
        assert!(!TextContext::is_locale("not a tag"));
        assert!(TextContext::is_locale("zh-Hant"));
        assert!(TextContext::is_script("Arab"));
        assert!(
            !TextContext::is_script("arab"),
            "spelled as ISO 15924 spells it"
        );
        assert!(!TextContext::is_script("Han"));
        assert!(!TextContext::preload_fallbacks("not a tag", &["Arab"]));
        assert!(!TextContext::preload_fallbacks("ko", &["Hang", "Han"]));
        assert_eq!(
            known_fallback("Hang", "ko"),
            None,
            "a refused preload asks for nothing"
        );
        assert!(TextContext::preload_fallbacks("en-US", &[]));
    }

    /// Setting a locale asks the platform for nothing: every script's list
    /// is filed empty, so a shaping whose characters the run's families all
    /// map asks for nothing either.
    #[test]
    fn nothing_is_asked_while_the_runs_own_families_cover_the_text() {
        let mut context = system_context_with_ahem("ko");
        assert!(fallback_of(&mut context, "Hang", Some("ko")).is_empty());
        assert!(fallback_of(&mut context, "Hani", None).is_empty());
        cover_in_ahem(&mut context, "Ahem covers this, \n and its space.");
        assert!(context.answered_scripts.is_empty());
        assert_eq!(known_fallback("Hang", "ko"), None);
    }

    /// A character the run's families lack fills its script's list with the
    /// platform's answer, from the process's one table, under the
    /// language's key and the language-less one, whatever the script.
    /// Skipped where the platform has no answer.
    #[test]
    fn a_missing_character_fills_its_scripts_list_from_the_process_table() {
        for (tag, script, text) in [
            ("zh-HK", "Hani", "今"),
            ("fa", "Arab", "ب"),
            ("th", "Thai", "ก"),
        ] {
            let mut context = system_context_with_ahem(tag);
            cover_in_ahem(&mut context, text);
            let known = known_fallback(script, tag).expect("asked once for the process");
            assert_eq!(fallback_of(&mut context, script, Some(tag)), known, "{tag}");
            assert_eq!(
                fallback_of(&mut context, script, None),
                known,
                "{tag}: the language-less key follows the language"
            );
            let mut later = system_context_with_ahem(tag);
            cover_in_ahem(&mut later, text);
            assert_eq!(
                fallback_of(&mut later, script, Some(tag)),
                known,
                "{tag}: reused"
            );
        }
    }

    /// A Common-script character is looked for only where fontique looks
    /// for it: the list it adds to every query, and its segment's script,
    /// one of its `Script_Extensions` the text around it is written in — not
    /// every script it may be written in, which for a CJK comma includes
    /// Hangul, Bopomofo and Yi.
    #[test]
    fn a_missing_common_character_asks_only_where_fontique_looks() {
        let mut context = system_context_with_ahem("zh-MO");
        cover_in_ahem(&mut context, "★");
        assert_eq!(context.answered_scripts, [FONTIQUE_EVERY_SEGMENT_SCRIPT]);
        let mut context = system_context_with_ahem("zh-MO");
        cover_in_ahem(&mut context, "abc，");
        assert_eq!(context.answered_scripts, [FONTIQUE_EVERY_SEGMENT_SCRIPT]);
        let mut context = system_context_with_ahem("th");
        cover_in_ahem(&mut context, "ก，");
        assert_eq!(
            context.answered_scripts,
            [
                parse_script("Thai").expect("a script"),
                FONTIQUE_EVERY_SEGMENT_SCRIPT
            ]
        );
    }

    /// A preload fills the process's table ahead of the first context to
    /// miss, which then takes the answer from there.
    #[test]
    fn a_preloaded_script_is_taken_from_the_table() {
        assert!(TextContext::preload_fallbacks("zh-SG", &["Hani"]));
        let known = known_fallback("Hani", "zh-SG").expect("preloaded");
        let mut context = system_context_with_ahem("zh-SG");
        cover_in_ahem(&mut context, "今");
        assert_eq!(fallback_of(&mut context, "Hani", Some("zh-SG")), known);
    }

    /// A context's own fallback containers answer for it alone, and a
    /// context without system fonts has no platform: neither files empty
    /// lists nor asks.
    #[test]
    fn only_a_context_the_platform_answers_asks_on_a_miss() {
        let mut containers = TextContext::new();
        assert_eq!(
            containers.register_fallback_fonts(FontBlob::from_static(AHEM)),
            1
        );
        assert!(containers.set_locale("ja"));
        assert!(!containers.fallback_on_miss);
        let ahem = containers
            .font
            .collection
            .family_id("Ahem")
            .expect("registered");
        assert_eq!(fallback_of(&mut containers, "Hani", Some("ja")), vec![ahem]);
        let mut no_system = TextContext::without_system_fonts();
        assert!(no_system.set_locale("ja"));
        assert!(!no_system.fallback_on_miss);
    }

    /// On a Mac the platform answers a Han cluster differently per language,
    /// and the language-less Han key, the one fontique appends to every
    /// query, follows the language set once a Han character is missing.
    /// Skipped where the platform does not have the faces.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_platform_han_fallback_follows_the_locale() {
        let family_names = |context: &mut TextContext, tag: Option<&str>| -> Vec<String> {
            let ids = fallback_of(context, "Hani", tag);
            ids.iter()
                .filter_map(|id| context.font.collection.family_name(*id).map(str::to_owned))
                .collect()
        };
        let mut traditional = system_context_with_ahem("zh-Hant");
        cover_in_ahem(&mut traditional, "今");
        let mut japanese = system_context_with_ahem("ja");
        cover_in_ahem(&mut japanese, "今");
        let traditional_han = family_names(&mut traditional, Some("zh-Hant"));
        let japanese_han = family_names(&mut japanese, Some("ja"));
        if traditional_han.is_empty() || japanese_han.is_empty() {
            return;
        }
        assert_ne!(traditional_han, japanese_han);
        assert_eq!(family_names(&mut traditional, None), traditional_han);
    }

    #[test]
    fn default_context_uses_the_system_constructor() {
        let context = TextContext::default();
        assert_eq!(context.shape_count(), 0);
        assert!(format!("{context:?}").starts_with("TextContext"));
    }

    #[test]
    fn preloading_builds_the_template_every_system_context_clones() {
        TextContext::preload_system_fonts();
        TextContext::preload_system_fonts();
        let families_from_another_thread = std::thread::spawn(|| {
            TextContext::preload_system_fonts();
            TextContext::new().font.collection.family_names().count()
        })
        .join()
        .expect("the preloading thread finishes");

        let mut context = TextContext::new();

        assert_eq!(
            context.font.collection.family_names().count(),
            families_from_another_thread,
            "a context built after a preload from another thread sees the same families"
        );
    }

    #[test]
    fn default_contexts_isolate_registered_fonts() {
        let mut context = TextContext::new();
        let mut sibling = TextContext::new();
        let sibling_before = sibling.font.collection.family_id("Ahem");

        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        assert!(context.font.collection.family_id("Ahem").is_some());
        assert_eq!(sibling.font.collection.family_id("Ahem"), sibling_before);
    }
}
