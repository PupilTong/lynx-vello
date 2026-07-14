//! Reusable Parley font and layout contexts.

use core::fmt;

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{FontContext, LayoutContext};

use crate::style::TextBrush;

/// Reusable resources for text shaping and layout.
///
/// Construct one per layout session (or other coarse host boundary) and lend
/// it mutably to node-scoped
/// [`TextMeasurer`](super::TextMeasurer) values. [`Self::new`] discovers
/// platform fonts; [`Self::without_system_fonts`] starts with an empty font
/// collection for deterministic tests and explicitly-managed applications.
pub struct TextContext {
    font: FontContext,
    layout: LayoutContext<TextBrush>,
    #[cfg(test)]
    shape_count: usize,
}

impl TextContext {
    /// Creates a context backed by the platform's system font collection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            font: FontContext::new(),
            layout: LayoutContext::new(),
            #[cfg(test)]
            shape_count: 0,
        }
    }

    /// Creates a context with no system fonts.
    ///
    /// Fonts registered later through [`Self::register_fonts`] are still
    /// available. This constructor makes text geometry independent of the
    /// machine running the layout.
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

    /// Registers every font face contained in `bytes`.
    ///
    /// The returned count is zero when the data contains no readable font
    /// faces. Registered bytes are retained by Parley's font collection.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.font
            .collection
            .register_fonts(Blob::from(bytes.to_vec()), None)
            .into_iter()
            .map(|(_, fonts)| fonts.len())
            .sum()
    }

    pub(super) fn parts(&mut self) -> (&mut FontContext, &mut LayoutContext<TextBrush>) {
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
}
