//! The generational arena backing the element tree.
//!
//! Elements live in a hand-rolled `Vec<Slot>` arena with a free list — no
//! `slotmap` dependency, deliberately minimal. Each element is addressed by an
//! [`ElementId`] carrying the slot index plus the slot's generation; once a
//! slot is freed its generation is bumped, so any [`ElementId`] referring to
//! the previous occupant becomes stale and resolves to `None`.
//!
//! [`ElementRef`] is a lightweight `Copy` handle pairing a borrow of the arena
//! with an [`ElementId`]; it exposes read-only tree navigation and is the type
//! stylo's element traits are implemented on (see [`crate::traits`]).

use std::num::NonZeroU32;

use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::element::Element;

/// The placeholder base URL for parsing a standalone arena's inline styles.
///
/// `about:blank` is a constant, valid URL, so this never fails.
fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// A stable, generation-checked handle to an element in an [`Arena`].
///
/// Cheap to copy and hash. A handle stays valid until its element is removed;
/// afterwards the slot's generation advances and the handle becomes stale
/// (arena lookups return `None`), even if the slot is later reused by a
/// different element.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ElementId {
    index: u32,
    generation: NonZeroU32,
}

impl ElementId {
    /// The slot index this handle refers to.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation this handle was minted with.
    #[must_use]
    pub const fn generation(self) -> NonZeroU32 {
        self.generation
    }
}

/// One arena slot: the current generation plus an optional live [`Element`].
#[derive(Debug)]
struct Slot<T> {
    generation: NonZeroU32,
    element: Option<Element<T>>,
}

/// A generational arena of [`Element`]s.
///
/// The arena owns the [`SharedRwLock`] and [`UrlExtraData`] used to parse and
/// guard every element's inline style block. The style engine driving the
/// cascade must resolve elements from an arena whose lock it *shares* (see
/// [`Arena::with_lock`]); otherwise stylo's `Locked::read_with` guard check
/// fails when the cascade reaches an inline block.
#[derive(Debug)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free_list: Vec<u32>,
    lock: SharedRwLock,
    url_data: UrlExtraData,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    /// Create an empty arena with a freshly minted [`SharedRwLock`] and a
    /// placeholder `about:blank` [`UrlExtraData`].
    ///
    /// A standalone arena (DOM-only, never styled) can use this. To style the
    /// tree, build it from an arena whose lock the style engine shares — see
    /// [`Arena::with_lock`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_lock(SharedRwLock::new(), about_blank_url_data())
    }

    /// Create an empty arena backed by an explicit [`SharedRwLock`] and
    /// [`UrlExtraData`], typically cloned from the style engine that will
    /// style this tree so their guards match.
    #[must_use]
    pub fn with_lock(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            lock,
            url_data,
        }
    }

    /// The shared lock guarding this arena's inline style blocks.
    #[must_use]
    pub fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    /// The base URL data used when parsing this arena's inline styles.
    #[must_use]
    pub fn url_data(&self) -> &UrlExtraData {
        &self.url_data
    }

    /// Insert an element and return its handle.
    ///
    /// # Panics
    ///
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn insert(&mut self, element: Element<T>) -> ElementId {
        if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.element.is_none(), "free-list slot must be vacant");
            slot.element = Some(element);
            ElementId {
                index,
                generation: slot.generation,
            }
        } else {
            let index =
                u32::try_from(self.slots.len()).expect("arena capacity exceeds u32::MAX slots");
            self.slots.push(Slot {
                generation: NonZeroU32::MIN,
                element: Some(element),
            });
            ElementId {
                index,
                generation: NonZeroU32::MIN,
            }
        }
    }

    /// Remove an element, returning it if the handle is live.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove(&mut self, id: ElementId) -> Option<Element<T>> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let element = slot.element.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free_list.push(id.index);
        } else {
            // Generation space exhausted for this slot: retire it (never
            // reuse) so no future handle can collide with a past one.
        }
        Some(element)
    }

    /// Borrow an element if the handle is live.
    #[must_use]
    pub fn get(&self, id: ElementId) -> Option<&Element<T>> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.element.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrow an element if the handle is live.
    pub fn get_mut(&mut self, id: ElementId) -> Option<&mut Element<T>> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.element.as_mut()
        } else {
            None
        }
    }

    /// Whether the handle currently resolves to a live element.
    #[must_use]
    pub fn contains(&self, id: ElementId) -> bool {
        self.get(id).is_some()
    }

    /// A read-only navigation handle for the element, if live.
    #[must_use]
    pub fn element_ref(&self, id: ElementId) -> Option<ElementRef<'_, T>> {
        if self.contains(id) {
            Some(ElementRef { arena: self, id })
        } else {
            None
        }
    }

    /// Clear every element's dirty bits.
    ///
    /// The style-flush driver calls this after a style-resolution pass; tests
    /// use it to establish a clean baseline before exercising invalidation.
    pub fn clear_dirty(&mut self) {
        for slot in &mut self.slots {
            if let Some(element) = &mut slot.element {
                element.style_dirty = false;
                element.dirty_descendants = false;
            }
        }
    }
}

/// A `Copy` read-only handle over an element and its arena, exposing tree
/// navigation.
///
/// This is the type stylo's element/traversal traits are implemented on (see
/// [`crate::traits`]); keeping it a thin `(&Arena, ElementId)` pair leaves that
/// seam clean. Only constructible via [`Arena::element_ref`], so the referenced
/// element is guaranteed live for the handle's (immutable) borrow of the arena.
pub struct ElementRef<'a, T> {
    pub(crate) arena: &'a Arena<T>,
    pub(crate) id: ElementId,
}

// Hand-written (rather than derived) so `T` needs no `Clone`/`Copy` bound: the
// handle only holds a shared reference plus an id.
impl<T> Clone for ElementRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ElementRef<'_, T> {}

impl<T> std::fmt::Debug for ElementRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let element = self.element();
        f.debug_struct("ElementRef")
            .field("id", &self.id)
            .field("tag", &element.tag_str())
            .finish()
    }
}

/// Two handles are equal when they point at the same element of the same arena.
///
/// stylo's `TElement`/`TNode` require `Eq`/`Hash`; identity is the arena pointer
/// paired with the [`ElementId`]. Comparing the arena by pointer (rather than by
/// value) keeps this cheap and matches stylo's expectation that element identity
/// is stable.
impl<T> PartialEq for ElementRef<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.id == other.id
    }
}

impl<T> Eq for ElementRef<'_, T> {}

impl<T> std::hash::Hash for ElementRef<'_, T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (std::ptr::from_ref(self.arena) as usize).hash(state);
        self.id.hash(state);
    }
}

impl<'a, T> ElementRef<'a, T> {
    /// Borrow the underlying element.
    ///
    /// `pub(crate)` so the [`traits`](crate::traits) impls can reach element
    /// state; the panic path is unreachable given the construction invariant
    /// (an `ElementRef` only exists for a live element).
    pub(crate) fn element(self) -> &'a Element<T> {
        self.arena
            .get(self.id)
            .expect("ElementRef always references a live element")
    }

    /// The handle for this element.
    #[must_use]
    pub const fn id(self) -> ElementId {
        self.id
    }

    /// The element's tag name.
    #[must_use]
    pub fn tag(self) -> &'a str {
        self.element().tag_str()
    }

    /// The element's external-state payload.
    #[must_use]
    pub fn ext(self) -> &'a T {
        &self.element().ext
    }

    /// The parent element, if any.
    #[must_use]
    pub fn parent(self) -> Option<ElementRef<'a, T>> {
        self.element()
            .parent
            .and_then(|p| self.arena.element_ref(p))
    }

    /// The first child element, if any.
    #[must_use]
    pub fn first_child(self) -> Option<ElementRef<'a, T>> {
        self.element()
            .children
            .first()
            .and_then(|&c| self.arena.element_ref(c))
    }

    /// The last child element, if any.
    #[must_use]
    pub fn last_child(self) -> Option<ElementRef<'a, T>> {
        self.element()
            .children
            .last()
            .and_then(|&c| self.arena.element_ref(c))
    }

    /// The next sibling element, if any.
    #[must_use]
    pub fn next_sibling(self) -> Option<ElementRef<'a, T>> {
        let parent = self.element().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let next = *siblings.get(pos + 1)?;
        self.arena.element_ref(next)
    }

    /// The previous sibling element, if any.
    #[must_use]
    pub fn prev_sibling(self) -> Option<ElementRef<'a, T>> {
        let parent = self.element().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let prev = *siblings.get(pos.checked_sub(1)?)?;
        self.arena.element_ref(prev)
    }

    /// Iterate over the element's children in document order.
    pub fn children(self) -> impl Iterator<Item = ElementRef<'a, T>> + 'a {
        let arena = self.arena;
        self.element()
            .children
            .iter()
            .filter_map(move |&id| arena.element_ref(id))
    }
}
