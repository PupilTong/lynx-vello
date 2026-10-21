//! Reusable Parley font and layout contexts.

use core::fmt;
use std::sync::OnceLock;

use parley::fontique::{
    Collection, CollectionOptions, FontInfoOverride, GenericFamily, Script, ScriptExt, SourceCache,
};
use parley::{FontContext, Layout, LayoutContext};

use super::FontBlob;
use crate::style::TextBrush;

fn system_font_template() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();

    TEMPLATE.get_or_init(FontContext::new)
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
            #[cfg(test)]
            shape_count: 0,
        }
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
        for &(script, _) in Script::all_samples() {
            self.font
                .collection
                .append_fallbacks(script, registered.iter().map(|(family, _)| *family));
        }
        registered.iter().map(|(_, fonts)| fonts.len()).sum()
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
    fn default_context_uses_the_system_constructor() {
        let context = TextContext::default();
        assert_eq!(context.shape_count(), 0);
        assert!(format!("{context:?}").starts_with("TextContext"));
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
