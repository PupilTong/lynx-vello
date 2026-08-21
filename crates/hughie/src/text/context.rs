//! Reusable Parley font and layout contexts.

use core::fmt;
use std::sync::OnceLock;

use parley::fontique::{Collection, CollectionOptions, GenericFamily, SourceCache};
use parley::{FontContext, LayoutContext};

use super::FontBlob;
use crate::style::TextBrush;

fn system_font_template() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();

    TEMPLATE.get_or_init(FontContext::new)
}

/// Reusable resources for text shaping and layout.
pub struct TextContext {
    font: FontContext,
    layout: LayoutContext<TextBrush>,
    #[cfg(test)]
    shape_count: usize,
}

impl TextContext {
    #[must_use]
    pub fn new() -> Self {
        Self {
            font: system_font_template().clone(),
            layout: LayoutContext::new(),
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

    pub(super) fn font_and_layout_contexts(
        &mut self,
    ) -> (&mut FontContext, &mut LayoutContext<TextBrush>) {
        (&mut self.font, &mut self.layout)
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
