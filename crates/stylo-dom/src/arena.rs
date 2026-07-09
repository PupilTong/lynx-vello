//! The generational arena backing the widget tree.
//!
//! Elements live in a hand-rolled `Vec<Slot>` arena with a free list — no
//! `slotmap` dependency, deliberately minimal. Each element is addressed by a
//! [`WidgetId`] carrying the slot index plus the slot's generation; once a slot
//! is freed its generation is bumped, so any [`WidgetId`] referring to the
//! previous occupant becomes stale and resolves to `None`.
//!
//! [`WidgetRef`] is a lightweight `Copy` handle pairing a borrow of the arena
//! with a [`WidgetId`]; it exposes read-only tree navigation and is the type
//! stylo's element traits are implemented on (see [`crate::traits`]).

use std::num::NonZeroU32;

use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::kind::WidgetKind;
use crate::widget::Widget;

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
pub struct WidgetId {
    index: u32,
    generation: NonZeroU32,
}

impl WidgetId {
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

/// One arena slot: the current generation plus an optional live [`Widget`].
#[derive(Debug)]
struct Slot {
    generation: NonZeroU32,
    widget: Option<Widget>,
}

/// A generational arena of [`Widget`]s.
///
/// Besides storage, the arena hands out the monotonically increasing Lynx
/// `unique_id` (`i32`) assigned to each created element (see
/// [`Widget::unique_id`]).
///
/// The arena also owns the [`SharedRwLock`] and [`UrlExtraData`] used to parse
/// and guard every element's inline style block. The `lynx-style`
/// [`StyleEngine`] must resolve elements from an arena whose lock it *shares*
/// (see [`Arena::with_lock`]); otherwise stylo's `Locked::read_with` guard
/// check fails when the cascade reaches an inline block.
///
/// [`StyleEngine`]: https://docs.rs/lynx-style
#[derive(Debug)]
pub struct Arena {
    slots: Vec<Slot>,
    free_list: Vec<u32>,
    next_unique_id: i32,
    lock: SharedRwLock,
    url_data: UrlExtraData,
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl Arena {
    /// Create an empty arena with a freshly minted [`SharedRwLock`] and a
    /// placeholder `about:blank` [`UrlExtraData`].
    ///
    /// A standalone arena (DOM-only, never styled) can use this. To style the
    /// tree, build it from an arena whose lock the `StyleEngine` shares — see
    /// [`Arena::with_lock`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_lock(SharedRwLock::new(), about_blank_url_data())
    }

    /// Create an empty arena backed by an explicit [`SharedRwLock`] and
    /// [`UrlExtraData`], typically cloned from the `StyleEngine` that will
    /// style this tree so their guards match.
    #[must_use]
    pub fn with_lock(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            // Lynx `unique_id`s are 1-based; 0 stays reserved as "unset".
            next_unique_id: 1,
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

    /// Insert a widget, assigning it the next Lynx `unique_id`, and return its
    /// handle.
    ///
    /// # Panics
    ///
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn insert(&mut self, mut widget: Widget) -> WidgetId {
        widget.unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);

        if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.widget.is_none(), "free-list slot must be vacant");
            slot.widget = Some(widget);
            WidgetId {
                index,
                generation: slot.generation,
            }
        } else {
            let index =
                u32::try_from(self.slots.len()).expect("arena capacity exceeds u32::MAX slots");
            self.slots.push(Slot {
                generation: NonZeroU32::MIN,
                widget: Some(widget),
            });
            WidgetId {
                index,
                generation: NonZeroU32::MIN,
            }
        }
    }

    /// Remove an element, returning its widget if the handle is live.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove(&mut self, id: WidgetId) -> Option<Widget> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let widget = slot.widget.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free_list.push(id.index);
        } else {
            // Generation space exhausted for this slot: retire it (never
            // reuse) so no future handle can collide with a past one.
        }
        Some(widget)
    }

    /// Borrow an element if the handle is live.
    #[must_use]
    pub fn get(&self, id: WidgetId) -> Option<&Widget> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.widget.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrow an element if the handle is live.
    pub fn get_mut(&mut self, id: WidgetId) -> Option<&mut Widget> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.widget.as_mut()
        } else {
            None
        }
    }

    /// Whether the handle currently resolves to a live element.
    #[must_use]
    pub fn contains(&self, id: WidgetId) -> bool {
        self.get(id).is_some()
    }

    /// A read-only navigation handle for the element, if live.
    #[must_use]
    pub fn widget_ref(&self, id: WidgetId) -> Option<WidgetRef<'_>> {
        if self.contains(id) {
            Some(WidgetRef { arena: self, id })
        } else {
            None
        }
    }

    /// Clear every element's dirty bits.
    ///
    /// The `lynx-style` crate calls this after a style-resolution pass; tests
    /// use it to establish a clean baseline before exercising invalidation.
    pub fn clear_dirty(&mut self) {
        for slot in &mut self.slots {
            if let Some(widget) = &mut slot.widget {
                widget.style_dirty = false;
                widget.dirty_descendants = false;
            }
        }
    }
}

/// A `Copy` read-only handle over an element and its arena, exposing tree
/// navigation.
///
/// This is the type stylo's element/traversal traits are implemented on (see
/// [`crate::traits`]); keeping it a thin `(&Arena, WidgetId)` pair leaves that
/// seam clean. Only constructible via [`Arena::widget_ref`], so the referenced
/// element is guaranteed live for the handle's (immutable) borrow of the arena.
#[derive(Clone, Copy)]
pub struct WidgetRef<'a> {
    pub(crate) arena: &'a Arena,
    pub(crate) id: WidgetId,
}

impl std::fmt::Debug for WidgetRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let widget = self.widget();
        f.debug_struct("WidgetRef")
            .field("id", &self.id)
            .field("kind", &widget.kind)
            .field("tag", &widget.tag_str())
            .finish()
    }
}

/// Two handles are equal when they point at the same element of the same arena.
///
/// stylo's `TElement`/`TNode` require `Eq`/`Hash`; identity is the arena pointer
/// paired with the [`WidgetId`]. Comparing the arena by pointer (rather than by
/// value) keeps this cheap and matches stylo's expectation that element identity
/// is stable.
impl PartialEq for WidgetRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.id == other.id
    }
}

impl Eq for WidgetRef<'_> {}

impl std::hash::Hash for WidgetRef<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (std::ptr::from_ref(self.arena) as usize).hash(state);
        self.id.hash(state);
    }
}

impl<'a> WidgetRef<'a> {
    /// Borrow the underlying widget.
    ///
    /// `pub(crate)` so the [`traits`](crate::traits) impls can reach widget
    /// state; the panic path is unreachable given the construction invariant (a
    /// `WidgetRef` only exists for a live element).
    pub(crate) fn widget(self) -> &'a Widget {
        self.arena
            .get(self.id)
            .expect("WidgetRef always references a live element")
    }

    /// The handle for this element.
    #[must_use]
    pub const fn id(self) -> WidgetId {
        self.id
    }

    /// The element's [`WidgetKind`].
    #[must_use]
    pub fn kind(self) -> WidgetKind {
        self.widget().kind
    }

    /// The element's Lynx tag name.
    #[must_use]
    pub fn tag(self) -> &'a str {
        self.widget().tag_str()
    }

    /// The element's Lynx `unique_id`.
    #[must_use]
    pub fn unique_id(self) -> i32 {
        self.widget().unique_id
    }

    /// The parent element, if any.
    #[must_use]
    pub fn parent(self) -> Option<WidgetRef<'a>> {
        self.widget().parent.and_then(|p| self.arena.widget_ref(p))
    }

    /// The first child element, if any.
    #[must_use]
    pub fn first_child(self) -> Option<WidgetRef<'a>> {
        self.widget()
            .children
            .first()
            .and_then(|&c| self.arena.widget_ref(c))
    }

    /// The last child element, if any.
    #[must_use]
    pub fn last_child(self) -> Option<WidgetRef<'a>> {
        self.widget()
            .children
            .last()
            .and_then(|&c| self.arena.widget_ref(c))
    }

    /// The next sibling element, if any.
    #[must_use]
    pub fn next_sibling(self) -> Option<WidgetRef<'a>> {
        let parent = self.widget().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let next = *siblings.get(pos + 1)?;
        self.arena.widget_ref(next)
    }

    /// The previous sibling element, if any.
    #[must_use]
    pub fn prev_sibling(self) -> Option<WidgetRef<'a>> {
        let parent = self.widget().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let prev = *siblings.get(pos.checked_sub(1)?)?;
        self.arena.widget_ref(prev)
    }

    /// Iterate over the element's children in document order.
    pub fn children(self) -> impl Iterator<Item = WidgetRef<'a>> + 'a {
        let arena = self.arena;
        self.widget()
            .children
            .iter()
            .filter_map(move |&id| arena.widget_ref(id))
    }
}
