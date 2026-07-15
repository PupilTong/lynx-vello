//! The single audited interior-mutable slot required by stylo.
//!
//! A style flush freezes the document and gives each element to exactly one
//! stylo worker. That worker may lazily create, mutate, or clear the element's
//! `ElementData` through a shared `&Node`. Outside a flush, the owner thread
//! reaches the slot only through `&mut Document` / `&mut Node`.

#![allow(
    unsafe_code,
    reason = "stylo's TElement contract requires per-element mutation through &Node"
)]

use std::cell::UnsafeCell;

use stylo::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};

/// The optional stylo data attached to one node.
///
/// Raw `UnsafeCell` access is intentionally confined to this type. Shared
/// access is sound only under stylo's one-worker-per-element traversal
/// discipline; exclusive owner-thread access goes through [`Self::get_mut`].
pub(crate) struct ElementDataSlot {
    inner: UnsafeCell<Option<ElementDataWrapper>>,
}

impl ElementDataSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }

    /// Create the slot if needed and mutably borrow its data.
    ///
    /// # Safety
    ///
    /// The caller must have Stylo-exclusive access to this element.
    pub(crate) unsafe fn ensure(&self) -> ElementDataMut<'_> {
        // SAFETY: forwarded from the method contract; no other worker accesses
        // this element's slot while the returned borrow is alive.
        let slot = unsafe { &mut *self.inner.get() };
        slot.get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    /// Remove all stylo data from this element.
    ///
    /// # Safety
    ///
    /// The caller must have Stylo-exclusive access to this element and no live
    /// borrow of its data.
    pub(crate) unsafe fn clear(&self) {
        // SAFETY: forwarded from the method contract.
        unsafe {
            *self.inner.get() = None;
        }
    }

    /// Whether the element currently has stylo data.
    pub(crate) fn is_initialized(&self) -> bool {
        // SAFETY: callers use this either outside a flush or from the worker
        // that owns this element. Creation/removal cannot race that access.
        unsafe { (*self.inner.get()).is_some() }
    }

    /// Immutably borrow initialized stylo data.
    pub(crate) fn borrow(&self) -> Option<ElementDataRef<'_>> {
        // SAFETY: the document phase and stylo's traversal discipline exclude
        // a concurrent mutable access to this element.
        unsafe { (*self.inner.get()).as_ref().map(ElementDataWrapper::borrow) }
    }

    /// Mutably borrow initialized stylo data from the owning worker.
    pub(crate) fn borrow_mut(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: stylo invokes this only for the worker that owns the element.
        unsafe {
            (*self.inner.get())
                .as_ref()
                .map(ElementDataWrapper::borrow_mut)
        }
    }

    /// Owner-thread access through an exclusive Node borrow.
    pub(crate) fn get_mut(&mut self) -> Option<&mut ElementDataWrapper> {
        self.inner.get_mut().as_mut()
    }

    /// Owner-thread creation through an exclusive Node borrow.
    pub(crate) fn get_or_insert_mut(&mut self) -> &mut ElementDataWrapper {
        self.inner
            .get_mut()
            .get_or_insert_with(ElementDataWrapper::default)
    }
}
