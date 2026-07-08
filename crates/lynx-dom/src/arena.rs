//! The generational arena backing the document tree.
//!
//! Elements live in a hand-rolled `Vec<Slot>` arena with a free list — no
//! `slotmap` dependency, deliberately minimal. Each element is addressed by an
//! [`ElementId`] carrying the slot index plus the slot's generation; once a
//! slot is freed its generation is bumped, so any [`ElementId`] referring to
//! the previous occupant becomes stale and resolves to `None`.
//!
//! [`ElemRef`] is a lightweight `Copy` handle pairing a borrow of the arena
//! with an [`ElementId`]; it exposes read-only tree navigation and is the type
//! stylo's element traits are implemented on in a later milestone.

use std::num::NonZeroU32;

use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::node::Node;
use crate::tag::NodeKind;

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

/// One arena slot: the current generation plus an optional live [`Node`].
#[derive(Debug)]
struct Slot {
    generation: NonZeroU32,
    node: Option<Node>,
}

/// A generational arena of [`Node`]s.
///
/// Besides storage, the arena hands out the monotonically increasing Lynx
/// `unique_id` (`i32`) assigned to each created element (see
/// [`Node::unique_id`]).
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

    /// Insert a node, assigning it the next Lynx `unique_id`, and return its
    /// handle.
    ///
    /// # Panics
    ///
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn insert(&mut self, mut node: Node) -> ElementId {
        node.unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);

        if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.node.is_none(), "free-list slot must be vacant");
            slot.node = Some(node);
            ElementId {
                index,
                generation: slot.generation,
            }
        } else {
            let index =
                u32::try_from(self.slots.len()).expect("arena capacity exceeds u32::MAX slots");
            self.slots.push(Slot {
                generation: NonZeroU32::MIN,
                node: Some(node),
            });
            ElementId {
                index,
                generation: NonZeroU32::MIN,
            }
        }
    }

    /// Remove an element, returning its node if the handle is live.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove(&mut self, id: ElementId) -> Option<Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let node = slot.node.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free_list.push(id.index);
        } else {
            // Generation space exhausted for this slot: retire it (never
            // reuse) so no future handle can collide with a past one.
        }
        Some(node)
    }

    /// Borrow an element if the handle is live.
    #[must_use]
    pub fn get(&self, id: ElementId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrow an element if the handle is live.
    pub fn get_mut(&mut self, id: ElementId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_mut()
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
    pub fn elem_ref(&self, id: ElementId) -> Option<ElemRef<'_>> {
        if self.contains(id) {
            Some(ElemRef { arena: self, id })
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
            if let Some(node) = &mut slot.node {
                node.style_dirty = false;
                node.dirty_descendants = false;
            }
        }
    }
}

/// A `Copy` read-only handle over an element and its arena, exposing tree
/// navigation.
///
/// This is the type stylo's element/traversal traits are implemented on in a
/// later milestone; keeping it a thin `(&Arena, ElementId)` pair leaves that
/// seam clean. Only constructible via [`Arena::elem_ref`], so the referenced
/// element is guaranteed live for the handle's (immutable) borrow of the
/// arena.
#[derive(Clone, Copy)]
pub struct ElemRef<'a> {
    pub(crate) arena: &'a Arena,
    pub(crate) id: ElementId,
}

impl std::fmt::Debug for ElemRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let node = self.node();
        f.debug_struct("ElemRef")
            .field("id", &self.id)
            .field("kind", &node.kind)
            .field("tag", &node.tag_str())
            .finish()
    }
}

/// Two handles are equal when they point at the same element of the same arena.
///
/// stylo's `TElement`/`TNode` require `Eq`/`Hash`; identity is the arena pointer
/// paired with the [`ElementId`]. Comparing the arena by pointer (rather than by
/// value) keeps this cheap and matches stylo's expectation that element identity
/// is stable.
impl PartialEq for ElemRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.id == other.id
    }
}

impl Eq for ElemRef<'_> {}

impl std::hash::Hash for ElemRef<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (std::ptr::from_ref(self.arena) as usize).hash(state);
        self.id.hash(state);
    }
}

impl<'a> ElemRef<'a> {
    /// Borrow the underlying node.
    ///
    /// `pub(crate)` so the `stylo_dom` trait impls can reach node state; the
    /// panic path is unreachable given the construction invariant (an
    /// `ElemRef` only exists for a live element).
    pub(crate) fn node(self) -> &'a Node {
        self.arena
            .get(self.id)
            .expect("ElemRef always references a live element")
    }

    /// The handle for this element.
    #[must_use]
    pub const fn id(self) -> ElementId {
        self.id
    }

    /// The element's [`NodeKind`].
    #[must_use]
    pub fn kind(self) -> NodeKind {
        self.node().kind
    }

    /// The element's Lynx tag name.
    #[must_use]
    pub fn tag(self) -> &'a str {
        self.node().tag_str()
    }

    /// The element's Lynx `unique_id`.
    #[must_use]
    pub fn unique_id(self) -> i32 {
        self.node().unique_id
    }

    /// The parent element, if any.
    #[must_use]
    pub fn parent(self) -> Option<ElemRef<'a>> {
        self.node().parent.and_then(|p| self.arena.elem_ref(p))
    }

    /// The first child element, if any.
    #[must_use]
    pub fn first_child(self) -> Option<ElemRef<'a>> {
        self.node()
            .children
            .first()
            .and_then(|&c| self.arena.elem_ref(c))
    }

    /// The last child element, if any.
    #[must_use]
    pub fn last_child(self) -> Option<ElemRef<'a>> {
        self.node()
            .children
            .last()
            .and_then(|&c| self.arena.elem_ref(c))
    }

    /// The next sibling element, if any.
    #[must_use]
    pub fn next_sibling(self) -> Option<ElemRef<'a>> {
        let parent = self.node().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let next = *siblings.get(pos + 1)?;
        self.arena.elem_ref(next)
    }

    /// The previous sibling element, if any.
    #[must_use]
    pub fn prev_sibling(self) -> Option<ElemRef<'a>> {
        let parent = self.node().parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == self.id)?;
        let prev = *siblings.get(pos.checked_sub(1)?)?;
        self.arena.elem_ref(prev)
    }

    /// Iterate over the element's children in document order.
    pub fn children(self) -> impl Iterator<Item = ElemRef<'a>> + 'a {
        let arena = self.arena;
        self.node()
            .children
            .iter()
            .filter_map(move |&id| arena.elem_ref(id))
    }
}
