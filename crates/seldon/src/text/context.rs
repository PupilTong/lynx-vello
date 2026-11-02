//! Reusable Parley font and layout contexts.

use core::fmt;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use parley::fontique::{
    Collection, CollectionOptions, FallbackKey, FamilyId, FontInfoOverride, GenericFamily,
    Language, Script, ScriptExt, SourceCache,
};
use parley::{FontContext, Layout, LayoutContext};

use super::FontBlob;
use crate::style::TextBrush;

fn system_font_template() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();

    TEMPLATE.get_or_init(FontContext::new)
}

/// A script and the language its fallback lookup names, `None` for a lookup
/// that names none: fontique's normalized key, without the bookkeeping its
/// `FallbackKey` adds, so that `zh-Hant` and `zh-TW` are one entry and a
/// language fontique does not distinguish for the script is no language.
type PreloadedKey = (Script, Option<Language>);

/// The platform's fallback for every script an embedder preloaded through
/// [`TextContext::preload_fallbacks`], in the language it named.
///
/// Fontique keeps what the platform answers in the context that asked, and
/// every context clones the template, so without this each one asks again.
/// What is asked is the embedder's choice; this keeps the answers for the
/// process. The families are the template's system families, the same in
/// every context cloned from it.
fn preloaded_fallbacks() -> &'static Mutex<HashMap<PreloadedKey, Vec<FamilyId>>> {
    static FALLBACKS: OnceLock<Mutex<HashMap<PreloadedKey, Vec<FamilyId>>>> = OnceLock::new();

    FALLBACKS.get_or_init(Mutex::default)
}

fn preloaded_key(script: Script, locale: Option<&Language>) -> PreloadedKey {
    (script, FallbackKey::new(script, locale).locale())
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
    /// [`preloaded_fallbacks`].
    system_fonts: bool,
    #[cfg(test)]
    shape_count: usize,
}

impl TextContext {
    #[must_use]
    pub fn new() -> Self {
        Self {
            font: system_font_template().clone(),
            layout: LayoutContext::new(),
            scratch: Layout::default(),
            locale: None,
            fallback_families: Vec::new(),
            system_fonts: true,
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
    /// codes such as `Hani` or `Arab`, in `locale`'s language, once for the
    /// whole process, so that no context asks for it again.
    ///
    /// The platform's fallback has costs fontique leaves where they fall:
    /// the first lookup in a process that names no language has CoreText
    /// build its whole default cascade list, about 8 ms on a Mac whatever
    /// the script, and a lookup that names a language is not cached by
    /// CoreText at all, so every context that makes it pays it again, up to
    /// a few hundred microseconds. Which scripts are worth paying for ahead
    /// of the first view is the embedder's to say: the answers are kept
    /// process-wide, and every context [`Self::set_locale`] sets to the same
    /// language takes them from there. `None` asks without a language, the
    /// lookup of a view that names none, whose answer CoreText keeps for the
    /// process itself once asked.
    ///
    /// Builds the template first, as [`Self::preload_system_fonts`] does.
    /// A script already answered for the language is not asked again.
    ///
    /// Returns `false`, having done nothing, when `locale` is not a
    /// well-formed BCP-47 language tag or a script is not one
    /// [`Self::is_script`] accepts.
    #[must_use]
    pub fn preload_fallbacks(locale: Option<&str>, scripts: &[&str]) -> bool {
        let locale = match locale.map(Language::parse) {
            None => None,
            Some(Ok(locale)) => Some(locale),
            Some(Err(_)) => return false,
        };
        let Some(scripts) = scripts
            .iter()
            .map(|code| parse_script(code))
            .collect::<Option<Vec<Script>>>()
        else {
            return false;
        };
        let mut context = Self::new();
        for script in scripts {
            let key = preloaded_key(script, locale.as_ref());
            let known = preloaded_fallbacks()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&key);
            if known {
                continue;
            }
            // Asked without the lock held: two calls that miss at once both
            // ask, and both answers are the same.
            let families: Vec<FamilyId> = context
                .font
                .collection
                .fallback_families(FallbackKey::new(script, locale.as_ref()))
                .collect();
            if !families.is_empty() {
                preloaded_fallbacks()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key, families);
            }
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
    /// The platform is asked for nothing here. Every script an embedder
    /// preloaded for the language through [`Self::preload_fallbacks`] is
    /// filed from that answer, both under the language's key and under the
    /// script's language-less key; the rest is asked the first time the
    /// context shapes it. The language-less key matters where fontique asks
    /// it in a run of another script: it appends the language-less Han
    /// fallback to every query so that Common-script punctuation in a run
    /// with no Han character still has a CJK face, so on a Traditional
    /// Chinese page whose embedder preloaded `Hani` that punctuation is set in
    /// the Traditional face rather than the platform's default. Families
    /// already registered through [`Self::register_fallback_fonts`] are filed
    /// under the language's keys instead and answer for this context alone,
    /// which then takes nothing preloaded and asks the platform for nothing,
    /// as under the language-less keys.
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
        self.adopt_preloaded_fallbacks(&locale);
        true
    }

    /// Files every preloaded answer for `locale`'s language, under the
    /// language's key and the script's language-less key, where the
    /// platform is the one to answer: this context has system fonts and no
    /// fallback container of its own, whose families would be the answer
    /// instead.
    fn adopt_preloaded_fallbacks(&mut self, locale: &Language) {
        if !self.system_fonts || !self.fallback_families.is_empty() {
            return;
        }
        let mut adopted: Vec<(Script, bool, Vec<FamilyId>)> = preloaded_fallbacks()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|((script, language), _)| preloaded_key(*script, Some(locale)).1 == *language)
            .map(|((script, language), families)| (*script, language.is_some(), families.clone()))
            .collect();
        // A language-less answer first, so that where a script has both, the
        // language's is the one its language-less key ends up with.
        adopted.sort_by_key(|(_, named, _)| *named);
        for (script, named, families) in adopted {
            if named {
                self.font.collection.set_fallbacks(
                    FallbackKey::new(script, Some(locale)),
                    families.iter().copied(),
                );
            }
            self.font
                .collection
                .set_fallbacks(script, families.into_iter());
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

    /// On a Mac the platform answers a Han cluster differently per language,
    /// and once the embedder preloads `Hani`, the language-less Han key, the
    /// one fontique appends to every query, follows the language set.
    /// Skipped where the platform does not have the faces.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_platform_han_fallback_follows_the_locale() {
        let han = Script::from_bytes(*b"Hani");
        let family_names = |context: &mut TextContext, tag: &str| -> Vec<String> {
            let language = Language::parse(tag).expect("a tag");
            let ids: Vec<_> = context
                .font
                .collection
                .fallback_families(FallbackKey::new(han, Some(&language)))
                .collect();
            ids.iter()
                .filter_map(|id| context.font.collection.family_name(*id).map(str::to_owned))
                .collect()
        };
        assert!(TextContext::preload_fallbacks(Some("zh-Hant"), &["Hani"]));
        assert!(TextContext::preload_fallbacks(Some("ja"), &["Hani"]));
        let mut traditional = TextContext::new();
        assert!(traditional.set_locale("zh-Hant"));
        let mut japanese = TextContext::new();
        assert!(japanese.set_locale("ja"));
        let traditional_han = family_names(&mut traditional, "zh-Hant");
        let japanese_han = family_names(&mut japanese, "ja");
        if traditional_han.is_empty() || japanese_han.is_empty() {
            return;
        }
        assert_ne!(traditional_han, japanese_han);
        let default_han: Vec<String> = traditional
            .font
            .collection
            .fallback_families(han)
            .collect::<Vec<_>>()
            .iter()
            .filter_map(|id| {
                traditional
                    .font
                    .collection
                    .family_name(*id)
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(default_han, traditional_han);
    }

    fn known_fallback(script: &str, tag: Option<&str>) -> Option<Vec<FamilyId>> {
        let language = tag.map(|tag| Language::parse(tag).expect("a tag"));
        let script = parse_script(script).expect("a script");
        preloaded_fallbacks()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&preloaded_key(script, language.as_ref()))
            .cloned()
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
        assert!(!TextContext::preload_fallbacks(
            Some("not a tag"),
            &["Arab"]
        ));
        assert!(!TextContext::preload_fallbacks(
            Some("ko"),
            &["Hang", "Han"]
        ));
        assert_eq!(
            known_fallback("Hang", Some("ko")),
            None,
            "a refused preload asks for nothing"
        );
        assert!(TextContext::preload_fallbacks(None, &[]));
    }

    /// Whatever scripts the embedder preloads, a context set to the language
    /// afterwards is answered from that, under the language's key and the
    /// script's language-less key, and whichever tag spelled the language.
    /// Skipped for a script the platform has no answer for.
    #[test]
    fn a_preloaded_script_serves_every_later_context() {
        for (tag, script) in [("zh-HK", "Hani"), ("fa", "Arab"), ("th", "Thai")] {
            assert!(TextContext::preload_fallbacks(Some(tag), &[script]));
            let Some(known) = known_fallback(script, Some(tag)) else {
                continue;
            };
            let mut context = TextContext::new();
            assert!(context.set_locale(tag));
            assert_eq!(fallback_of(&mut context, script, Some(tag)), known, "{tag}");
            assert_eq!(
                fallback_of(&mut context, script, None),
                known,
                "{tag}: the language-less key follows the language"
            );
        }
    }

    /// Setting a locale asks the platform for nothing and leaves nothing for
    /// the process; what is preloaded is the embedder's choice.
    #[test]
    fn a_locale_alone_preloads_nothing() {
        let mut context = TextContext::new();
        assert!(context.set_locale("ko"));
        assert_eq!(known_fallback("Hani", Some("ko")), None);
        assert_eq!(known_fallback("Hang", Some("ko")), None);
    }

    /// A context's own fallback containers answer for it alone, and a
    /// context without system fonts has no platform: neither takes a
    /// preloaded answer.
    #[test]
    fn only_a_context_the_platform_answers_takes_a_preloaded_answer() {
        assert!(TextContext::preload_fallbacks(Some("zh-SG"), &["Hani"]));
        let mut containers = TextContext::new();
        assert_eq!(
            containers.register_fallback_fonts(FontBlob::from_static(AHEM)),
            1
        );
        assert!(containers.set_locale("zh-SG"));
        let ahem = containers
            .font
            .collection
            .family_id("Ahem")
            .expect("registered");
        assert_eq!(
            fallback_of(&mut containers, "Hani", Some("zh-SG")),
            vec![ahem]
        );
        let mut no_system = TextContext::without_system_fonts();
        assert!(no_system.set_locale("zh-SG"));
        assert!(fallback_of(&mut no_system, "Hani", Some("zh-SG")).is_empty());
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
