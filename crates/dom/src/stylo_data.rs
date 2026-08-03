//! [`StyloData`] — an element's slot for Stylo's own [`ElementData`].
//!
//! This module is the single place in the crate that reaches into the slot's
//! interior mutability, so no other module needs a raw `unsafe` block to read
//! or write an element's style data.
//!
//! [`ElementData`]: stylo::data::ElementData
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::fmt;

use stylo::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};

/// Per-element storage for Stylo's `ElementData`, plus the "not styled yet"
/// state Stylo models as the container being absent.
///
/// Every `TElement` data accessor takes `&self` — including `ensure_data` and
/// `clear_data`, which create and destroy the container — so the slot has to
/// be interior-mutable. Stylo's own [`ElementDataWrapper`] already
/// runtime-checks borrows of the `ElementData` *inside* it under
/// `debug_assertions`; what no wrapper can cover is the `Option` *around* it,
/// which `ensure_data`/`clear_data` overwrite. Soundness there rests on
/// Stylo's traversal ownership contract — one worker owns an element while it
/// is being styled, which is exactly why those two trait methods are `unsafe`
/// — so this type states that contract as an `unsafe fn` boundary instead of
/// re-checking it at runtime. Servo, Blitz and Paws all wire the type this
/// way.
///
/// The safe accessors cover everything outside a style traversal: shared reads
/// through `&self`, exclusive writes through `&mut self`.
pub(crate) struct StyloData {
    inner: UnsafeCell<Option<ElementDataWrapper>>,
}

impl StyloData {
    pub(crate) const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }

    /// Whether Stylo has styled this element at least once.
    pub(crate) fn has_data(&self) -> bool {
        // SAFETY: reads the `Option` discriminant only. A concurrent
        // `ensure_init`/`clear` would be a violation of Stylo's exclusive
        // access contract on its own terms.
        unsafe { &*self.inner.get() }.is_some()
    }

    /// Borrows the data immutably, if it has been initialized.
    pub(crate) fn borrow(&self) -> Option<ElementDataRef<'_>> {
        // SAFETY: as in `has_data`; the borrow of the `ElementData` itself is
        // tracked by `ElementDataWrapper`.
        unsafe { &*self.inner.get() }
            .as_ref()
            .map(ElementDataWrapper::borrow)
    }

    /// Borrows the data mutably, if it has been initialized.
    pub(crate) fn borrow_mut(&mut self) -> Option<ElementDataMut<'_>> {
        self.inner
            .get_mut()
            .as_ref()
            .map(ElementDataWrapper::borrow_mut)
    }

    /// [`Self::borrow_mut`] through a shared reference, for the `&self`
    /// receiver Stylo's `TElement::mutate_data` hands us.
    ///
    /// # Safety
    ///
    /// The caller must hold Stylo's exclusive access to this element, so that
    /// the slot is not being initialized or cleared concurrently.
    pub(crate) unsafe fn borrow_mut_unchecked(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: the caller guarantees exclusive access to the slot.
        unsafe { &*self.inner.get() }
            .as_ref()
            .map(ElementDataWrapper::borrow_mut)
    }

    /// Initializes the container if it is absent, and borrows it mutably.
    ///
    /// # Safety
    ///
    /// The caller must hold Stylo's exclusive access to this element, and no
    /// borrow of this slot may be outstanding.
    pub(crate) unsafe fn ensure_init(&self) -> ElementDataMut<'_> {
        // SAFETY: the caller guarantees exclusive access to the slot.
        unsafe { &mut *self.inner.get() }
            .get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    /// Drops the container, returning the element to its unstyled state.
    ///
    /// # Safety
    ///
    /// The caller must hold Stylo's exclusive access to this element, and no
    /// borrow of this slot may be outstanding — clearing destroys the
    /// `ElementDataWrapper` that would otherwise track them.
    pub(crate) unsafe fn clear(&self) {
        // SAFETY: the caller guarantees exclusive access to the slot.
        unsafe { *self.inner.get() = None };
    }
}

impl Default for StyloData {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StyloData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyloData")
            .field("has_data", &self.has_data())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use stylo::invalidation::element::restyle_hints::RestyleHint;

    use super::StyloData;

    /// `ensure_init` is idempotent, and `clear` returns the slot to its
    /// unstyled state rather than resetting the data in place.
    #[test]
    fn init_is_idempotent_and_clear_drops_the_container() {
        let mut slot = StyloData::new();
        assert!(!slot.has_data());
        assert!(slot.borrow().is_none());
        assert!(slot.borrow_mut().is_none());

        // SAFETY: `&mut slot` is the strongest form of the exclusive access
        // Stylo's traversal contract asks for, and no borrow is outstanding.
        unsafe { slot.ensure_init() }.hint = RestyleHint::RESTYLE_SELF;
        assert!(slot.has_data());
        assert!(
            // SAFETY: as above.
            unsafe { slot.ensure_init() }
                .hint
                .contains(RestyleHint::RESTYLE_SELF),
            "re-initializing an initialized slot must keep the existing container"
        );

        // SAFETY: as above.
        unsafe { slot.clear() };
        assert!(!slot.has_data());
        assert!(slot.borrow().is_none());
        assert!(
            // SAFETY: as above.
            unsafe { slot.ensure_init() }.hint.is_empty(),
            "clearing must drop the container, not reset it in place"
        );
    }

    /// The debug-only borrow tracking this type relies on lives inside Stylo's
    /// `ElementDataWrapper`; check it is actually reached through our
    /// accessors, since nothing in this crate re-checks it.
    #[cfg(debug_assertions)]
    #[test]
    fn a_writer_is_rejected_while_a_reader_is_live() {
        let slot = StyloData::new();
        // SAFETY: nothing else can reference a slot local to this test.
        drop(unsafe { slot.ensure_init() });

        let _reader = slot.borrow().expect("the slot was just initialized");
        let conflict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: as above — the point is that Stylo still catches it.
            drop(unsafe { slot.borrow_mut_unchecked() });
        }));
        assert!(
            conflict.is_err(),
            "Stylo must reject a writer while a reader holds the ElementData"
        );
    }
}
