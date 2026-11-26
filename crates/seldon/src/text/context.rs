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

const FONTIQUE_EVERY_SEGMENT_SCRIPT: Script = Script::from_bytes(*b"Hani");

type PlatformKey = (Script, Option<Language>);

fn platform_fallbacks() -> &'static Mutex<HashMap<PlatformKey, Arc<[FamilyId]>>> {
    static FALLBACKS: OnceLock<Mutex<HashMap<PlatformKey, Arc<[FamilyId]>>>> = OnceLock::new();

    FALLBACKS.get_or_init(Mutex::default)
}

fn platform_key(script: Script, locale: &Language) -> PlatformKey {
    (script, FallbackKey::new(script, Some(locale)).locale())
}

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

fn needs_no_glyph(ch: char) -> bool {
    CodePointSetData::new::<WhiteSpace>().contains(ch)
        || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(ch)
        || CodePointSetData::new::<EmojiPresentation>().contains(ch)
        || CodePointMapData::<GeneralCategory>::new().get(ch) == GeneralCategory::Control
}

fn parse_script(code: &str) -> Option<Script> {
    let bytes: [u8; 4] = code.as_bytes().try_into().ok()?;
    let script = Script::from_bytes(bytes);
    script.sample().is_some().then_some(script)
}

const SCRATCH_KEEP_BYTES: usize = 16 * 1024;

pub struct TextContext {
    font: FontContext,
    layout: LayoutContext<TextBrush>,
    scratch: Layout<TextBrush>,
    locale: Option<Language>,
    fallback_families: Vec<FamilyId>,
    system_fonts: bool,
    fallback_on_miss: bool,
    answered_scripts: Vec<Script>,
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
            fallback_on_miss: false,
            answered_scripts: Vec::new(),
            #[cfg(test)]
            shape_count: 0,
        }
    }

    pub fn preload_system_fonts() {
        let _ = system_font_template();
    }

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

    #[must_use]
    pub fn is_locale(tag: &str) -> bool {
        Language::parse(tag).is_ok()
    }

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

    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        self.font
            .collection
            .register_fonts(data.into_inner(), None)
            .into_iter()
            .map(|(_, fonts)| fonts.len())
            .sum()
    }

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

    #[must_use]
    pub fn locale(&self) -> Option<Language> {
        self.locale
    }

    pub(super) fn last_resort_family(&mut self) -> Option<String> {
        let collection = &mut self.font.collection;
        if collection
            .generic_families(GenericFamily::SansSerif)
            .next()
            .is_some()
        {
            return None;
        }
        collection.family_names().next().map(str::to_owned)
    }

    pub(super) fn shaping_parts(
        &mut self,
    ) -> (
        &mut FontContext,
        &mut LayoutContext<TextBrush>,
        &mut Layout<TextBrush>,
    ) {
        (&mut self.font, &mut self.layout, &mut self.scratch)
    }

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

    #[test]
    fn nothing_is_asked_while_the_runs_own_families_cover_the_text() {
        let mut context = system_context_with_ahem("ko");
        assert_eq!(fallback_of(&mut context, "Hang", Some("ko")), []);
        assert_eq!(fallback_of(&mut context, "Hani", None), []);
        cover_in_ahem(&mut context, "Ahem covers this, \n and its space.");
        assert_eq!(context.answered_scripts, []);
        assert_eq!(known_fallback("Hang", "ko"), None);
    }

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

    #[test]
    fn a_preloaded_script_is_taken_from_the_table() {
        assert!(TextContext::preload_fallbacks("zh-SG", &["Hani"]));
        let known = known_fallback("Hani", "zh-SG").expect("preloaded");
        let mut context = system_context_with_ahem("zh-SG");
        cover_in_ahem(&mut context, "今");
        assert_eq!(fallback_of(&mut context, "Hani", Some("zh-SG")), known);
    }

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
