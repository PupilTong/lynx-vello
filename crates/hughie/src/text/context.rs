//! Reusable Parley font and layout contexts.

use core::fmt;
use std::sync::OnceLock;

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{FontContext, LayoutContext};

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
            // The discovered system-font backend is immutable and shared;
            // mutable source and layout caches remain local to this context.
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

    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.font
            .collection
            .register_fonts(Blob::from(bytes.to_vec()), None)
            .into_iter()
            .map(|(_, fonts)| fonts.len())
            .sum()
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
        assert_eq!(context.register_fonts(b"not a font"), 0);
        assert_eq!(context.register_fonts(AHEM), 1);
        assert!(context.font.collection.family_id("Ahem").is_some());
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

        assert_eq!(context.register_fonts(AHEM), 1);
        assert!(context.font.collection.family_id("Ahem").is_some());
        assert_eq!(sibling.font.collection.family_id("Ahem"), sibling_before);
    }
}
