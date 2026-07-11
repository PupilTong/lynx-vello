//! Inline-style parsing on the [`Arena`].
//!
//! These helpers parse `style`-attribute CSS text into a stylo
//! [`PropertyDeclarationBlock`] guarded by the arena's
//! [`SharedRwLock`](stylo::shared_lock::SharedRwLock). The parsing lives here —
//! alongside the arena that owns the lock and base [`UrlExtraData`] — so the
//! embedder's API layer just validates the target handle and delegates. Both
//! helpers apply the standard attribute-change invalidation (see
//! [`crate::dirty`]).

use stylo::context::QuirksMode;
use stylo::properties::declaration_block::{parse_one_declaration_into, parse_style_attribute};
use stylo::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
};
use stylo::servo_arc::Arc;
use stylo::stylesheets::{CssRuleType, Origin};
use stylo_traits::ParsingMode;

use crate::arena::{Arena, ElementId};

impl<T> Arena<T> {
    /// Count parsed declarations in an element's inline style.
    ///
    /// Returns `None` for a stale handle and `Some(0)` when the element has no
    /// inline block. This keeps the style lock encapsulated while still
    /// allowing diagnostics and tests to inspect parsed state.
    #[must_use]
    pub fn inline_style_declaration_count(&self, id: ElementId) -> Option<usize> {
        let element = self.get(id)?;
        let Some(block) = element.inline_block.as_ref() else {
            return Some(0);
        };
        let guard = self.shared_lock().read();
        Some(block.read_with(&guard).declarations().len())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (the `style` attribute). An empty string clears it.
    ///
    /// Assumes `id` is live (the embedder validates it); a stale handle is a
    /// silent no-op here.
    pub fn set_inline_styles(&mut self, id: ElementId, text: &str) {
        let block = if text.is_empty() {
            None
        } else {
            let parsed = parse_style_attribute(
                text,
                self.url_data(),
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            );
            Some(Arc::new(self.shared_lock().wrap(parsed)))
        };
        if let Some(element) = self.get_mut(id) {
            element.inline_block = block;
        }
        self.note_inline_style_change(id);
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block.
    ///
    /// Mirrors Paws' `update_inline_style`: only the one new declaration is
    /// parsed and folded into a clone of the existing block, avoiding a
    /// whole-attribute re-parse. An unparseable property/value is dropped.
    ///
    /// Assumes `id` is live (the embedder validates it).
    pub fn add_inline_style(&mut self, id: ElementId, name: &str, value: &str) {
        let Ok(property_id) = PropertyId::parse_unchecked(name, None) else {
            // Unknown non-custom property: drop it (no debug logging yet).
            return;
        };

        let mut source = SourcePropertyDeclaration::default();
        if parse_one_declaration_into(
            &mut source,
            property_id,
            value,
            Origin::Author,
            self.url_data(),
            None,
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        )
        .is_err()
        {
            return;
        }

        let lock = self.shared_lock();
        let mut block = match self
            .get(id)
            .and_then(|element| element.inline_block.as_ref())
        {
            Some(existing) => {
                let guard = lock.read();
                existing.read_with(&guard).clone()
            }
            None => PropertyDeclarationBlock::new(),
        };
        block.extend(source.drain(), Importance::Normal);
        let wrapped = Arc::new(lock.wrap(block));

        if let Some(element) = self.get_mut(id) {
            element.inline_block = Some(wrapped);
        }
        self.note_inline_style_change(id);
    }
}
