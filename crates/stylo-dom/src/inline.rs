//! Inline-style parsing on the [`Arena`].
//!
//! Lynx's `__SetInlineStyles` / `__AddInlineStyle` opcodes parse CSS text into a
//! stylo [`PropertyDeclarationBlock`] guarded by the arena's
//! [`SharedRwLock`](stylo::shared_lock::SharedRwLock). The parsing lives here —
//! alongside the arena that owns the lock and base [`UrlExtraData`] — so the
//! PAPI layer just validates the target handle and delegates. Both helpers apply
//! the standard attribute-change invalidation (see [`crate::dirty`]).

use stylo::context::QuirksMode;
use stylo::properties::declaration_block::{parse_one_declaration_into, parse_style_attribute};
use stylo::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
};
use stylo::servo_arc::Arc;
use stylo::stylesheets::{CssRuleType, Origin};
use stylo_traits::ParsingMode;

use crate::arena::{Arena, WidgetId};

impl Arena {
    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    ///
    /// Assumes `id` is live (the PAPI layer validates it); a stale handle is a
    /// silent no-op here.
    pub fn set_inline_styles(&mut self, id: WidgetId, text: &str) {
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
        if let Some(widget) = self.get_mut(id) {
            widget.inline_block = block;
        }
        self.mark_attribute_changed(id);
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// Mirrors Paws' `update_inline_style`: only the one new declaration is
    /// parsed and folded into a clone of the existing block, avoiding a
    /// whole-attribute re-parse. An unparseable property/value is dropped.
    ///
    /// Assumes `id` is live (the PAPI layer validates it).
    pub fn add_inline_style(&mut self, id: WidgetId, name: &str, value: &str) {
        let Ok(property_id) = PropertyId::parse_unchecked(name, None) else {
            // Unknown non-custom property: drop it (M2 has no debug logging yet).
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
        let mut block = match self.get(id).and_then(|widget| widget.inline_block.as_ref()) {
            Some(existing) => {
                let guard = lock.read();
                existing.read_with(&guard).clone()
            }
            None => PropertyDeclarationBlock::new(),
        };
        block.extend(source.drain(), Importance::Normal);
        let wrapped = Arc::new(lock.wrap(block));

        if let Some(widget) = self.get_mut(id) {
            widget.inline_block = Some(wrapped);
        }
        self.mark_attribute_changed(id);
    }
}
