//! The Element PAPI as data: calls recorded beside the script, applied by
//! whatever owns the [`ElementTree`].
//!
//! The Element PAPI is a write protocol whose only commit point is
//! `__FlushElementTree`, so a script never observes tree state between
//! writes — everything a call returns (unique ids, validation errors) is
//! derivable from the call sequence alone. That is what makes this split
//! sound: an [`ElementOpRecorder`] answers every call immediately from a
//! shadow of the tree and records the write as an [`ElementOp`], and the
//! engine that owns the real [`ElementTree`] applies the recorded batch at
//! the flush boundary — on the same thread (the headless composition) or on
//! another one (the windowed shell), without the script side ever holding
//! the tree.
//!
//! # The mirroring law
//!
//! The recorder must answer exactly what [`ElementTree`] would have
//! answered: the same unique ids (the arena never recycles, so both sides
//! allocate with the same monotonic counter) and the same [`PapiError`]s in
//! the same precedence order. [`ElementTree::apply`] asserts the id
//! lockstep and re-runs the tree's own validation, so a divergence is a loud
//! failure at the commit, never a silently wrong tree. A change to any PAPI
//! method on [`ElementTree`] must change [`ElementOpRecorder`] in the same
//! commit.

use crate::ElementId;
use crate::tree::{ElementTree, PAGE_UNIQUE_ID, PapiError};

/// One recorded Element PAPI write, in the vocabulary the ids already speak.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElementOp {
    /// `__CreatePage(componentID, componentCSSID)` — recorded only for the
    /// first, binding call; repeats return the page id without a write.
    CreatePage {
        component_id: String,
        component_css_id: i32,
    },
    /// `__CreateView(parentComponentUniqueID)`. `id` is the unique id the
    /// recorder allocated and already handed to the script; the applying
    /// tree asserts it allocates the same one.
    CreateView {
        id: ElementId,
        parent_component_unique_id: ElementId,
    },
    /// `__AppendElement(parent, child)`.
    AppendElement { parent: ElementId, child: ElementId },
    /// `__DropElement(element)` — only recorded when the drop retired a live
    /// element; unknown-handle drops are answered without a write.
    DropElement { id: ElementId },
}

/// The structural facts the recorder needs to answer a call: liveness, the
/// parent link (for cycle checks), and the children (for subtree drops).
#[derive(Debug)]
struct ShadowElement {
    /// The element parent, or `0` while detached. The page's parent is the
    /// document node, which no element id names, so it stays `0` forever.
    parent: ElementId,
    children: Vec<ElementId>,
}

impl ShadowElement {
    const fn detached() -> Self {
        Self {
            parent: 0,
            children: Vec::new(),
        }
    }
}

/// The script-side half of the Element PAPI: a shadow of the element tree
/// that validates and answers every call immediately and records the writes
/// for the tree owner to [`ElementTree::apply`] at the next flush.
#[derive(Debug)]
pub struct ElementOpRecorder {
    /// Mirror of the tree's permanent-index arena: index = unique id, slot
    /// zero is the "no element" sentinel, retirement tombstones forever.
    slots: Vec<Option<ShadowElement>>,
    page_created: bool,
    ops: Vec<ElementOp>,
}

impl Default for ElementOpRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl ElementOpRecorder {
    /// A recorder over a fresh tree. The page element pre-exists in the
    /// document, so its shadow slot is live from birth too.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: vec![None, Some(ShadowElement::detached())],
            page_created: false,
            ops: Vec::new(),
        }
    }

    /// `__CreatePage(componentID, componentCSSID)` — idempotent, like
    /// [`ElementTree::create_page`].
    pub fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId {
        if !self.page_created {
            self.page_created = true;
            self.ops.push(ElementOp::CreatePage {
                component_id: component_id.to_owned(),
                component_css_id,
            });
        }
        PAGE_UNIQUE_ID
    }

    /// `__CreateView(parentComponentUniqueID)`, mirroring
    /// [`ElementTree::create_view`]'s validation and id allocation.
    pub fn create_view(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        if parent_component_unique_id != 0 && !self.is_live(parent_component_unique_id) {
            return Err(PapiError::UnknownElement(parent_component_unique_id));
        }
        let id = u32::try_from(self.slots.len())
            .expect("a Lynx element recorder exhausted its u32 unique ids");
        self.slots.push(Some(ShadowElement::detached()));
        self.ops.push(ElementOp::CreateView {
            id,
            parent_component_unique_id,
        });
        Ok(id)
    }

    /// `__AppendElement(parent, child)`, mirroring
    /// [`ElementTree::append_element`]'s checks in the same order: unknown
    /// parent, unknown child, the page guard, then cycles.
    pub fn append_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, PapiError> {
        if !self.is_live(parent) {
            return Err(PapiError::UnknownElement(parent));
        }
        if !self.is_live(child) {
            return Err(PapiError::UnknownElement(child));
        }
        if child == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotReparentPage);
        }
        if parent == child || self.is_strict_ancestor(child, parent) {
            return Err(PapiError::WouldCycle { parent, child });
        }

        let old_parent = self.slot(child).parent;
        if old_parent != 0 {
            self.slot_mut(old_parent)
                .children
                .retain(|&sibling| sibling != child);
        }
        self.slot_mut(child).parent = parent;
        self.slot_mut(parent).children.push(child);
        self.ops.push(ElementOp::AppendElement { parent, child });
        Ok(child)
    }

    /// `__DropElement(element)`, mirroring [`ElementTree::drop_element`]:
    /// the page guard precedes the liveness check, and a successful drop
    /// retires the whole subtree's ids forever.
    pub fn drop_element(&mut self, id: ElementId) -> Result<(), PapiError> {
        if id == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        if !self.is_live(id) {
            return Err(PapiError::UnknownElement(id));
        }

        let parent = self.slot(id).parent;
        if parent != 0 {
            self.slot_mut(parent)
                .children
                .retain(|&sibling| sibling != id);
        }
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let slot = self.slots[arena_index(current)]
                .take()
                .expect("a shadow subtree member is live until retired here");
            stack.extend(slot.children);
        }
        self.ops.push(ElementOp::DropElement { id });
        Ok(())
    }

    /// Drains the writes recorded since the last drain — the batch one
    /// `__FlushElementTree` commits.
    pub fn take_ops(&mut self) -> Vec<ElementOp> {
        std::mem::take(&mut self.ops)
    }

    fn is_live(&self, id: ElementId) -> bool {
        id != 0 && self.slots.get(arena_index(id)).is_some_and(Option::is_some)
    }

    /// Whether `candidate` is a proper ancestor of `of`, walking the shadow
    /// parent links. Equality is the caller's separate check, matching the
    /// tree's `parent == child ||` shape.
    fn is_strict_ancestor(&self, candidate: ElementId, of: ElementId) -> bool {
        let mut current = self.slot(of).parent;
        while current != 0 {
            if current == candidate {
                return true;
            }
            current = self.slot(current).parent;
        }
        false
    }

    fn slot(&self, id: ElementId) -> &ShadowElement {
        self.slots[arena_index(id)]
            .as_ref()
            .expect("shadow slots are checked live before structural access")
    }

    fn slot_mut(&mut self, id: ElementId) -> &mut ShadowElement {
        self.slots[arena_index(id)]
            .as_mut()
            .expect("shadow slots are checked live before structural access")
    }
}

fn arena_index(id: ElementId) -> usize {
    usize::try_from(id).expect("a u32 unique id indexes the shadow arena")
}

impl ElementTree {
    /// Applies one recorded op to the live tree.
    ///
    /// The recorder already validated the call against its shadow, so an
    /// `Err` means the shadow and the tree have diverged — the commit layer
    /// must reject the batch loudly rather than half-trust it. The unique-id
    /// lockstep is asserted outright: both allocators are monotonic and
    /// never recycle, so a mismatch is a bug, not an input.
    pub fn apply(&mut self, op: &ElementOp) -> Result<(), PapiError> {
        match op {
            ElementOp::CreatePage {
                component_id,
                component_css_id,
            } => {
                self.create_page(component_id, *component_css_id);
                Ok(())
            }
            ElementOp::CreateView {
                id,
                parent_component_unique_id,
            } => {
                let created = self.create_view(*parent_component_unique_id)?;
                assert_eq!(
                    created, *id,
                    "the recorder and the element arena must allocate unique ids in lockstep"
                );
                Ok(())
            }
            ElementOp::AppendElement { parent, child } => {
                self.append_element(*parent, *child).map(|_| ())
            }
            ElementOp::DropElement { id } => self.drop_element(*id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementOp, ElementOpRecorder, ElementTree, PapiError};
    use crate::device::Viewport;
    use crate::ua::PageConfig;

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    /// Drives the same call through the recorder and a directly-mutated tree,
    /// asserting both give the same answer — the mirroring law, mechanized.
    struct Mirror {
        recorder: ElementOpRecorder,
        direct: ElementTree,
    }

    impl Mirror {
        fn new() -> Self {
            Self {
                recorder: ElementOpRecorder::new(),
                direct: tree(),
            }
        }

        fn create_page(&mut self, component_id: &str, component_css_id: i32) -> u32 {
            let recorded = self.recorder.create_page(component_id, component_css_id);
            let direct = self.direct.create_page(component_id, component_css_id);
            assert_eq!(recorded, direct, "create_page diverged");
            recorded
        }

        fn create_view(&mut self, parent_component: u32) -> Result<u32, PapiError> {
            let recorded = self.recorder.create_view(parent_component);
            let direct = self.direct.create_view(parent_component);
            assert_eq!(recorded, direct, "create_view diverged");
            recorded
        }

        fn append_element(&mut self, parent: u32, child: u32) -> Result<u32, PapiError> {
            let recorded = self.recorder.append_element(parent, child);
            let direct = self.direct.append_element(parent, child);
            assert_eq!(recorded, direct, "append_element diverged");
            recorded
        }

        fn drop_element(&mut self, id: u32) -> Result<(), PapiError> {
            let recorded = self.recorder.drop_element(id);
            let direct = self.direct.drop_element(id);
            assert_eq!(recorded, direct, "drop_element diverged");
            recorded
        }

        /// Applies the recorded ops to a fresh tree and asserts it reaches
        /// the same structure as the directly-mutated one.
        fn assert_replay_matches(mut self) {
            let mut replayed = tree();
            for op in self.recorder.take_ops() {
                replayed
                    .apply(&op)
                    .expect("recorder-validated ops must apply cleanly");
            }
            let ids = 0..u32::try_from(self.recorder.slots.len()).unwrap() + 2;
            for id in ids {
                let direct_node = self.direct.node_id(id);
                let replayed_node = replayed.node_id(id);
                assert_eq!(
                    direct_node.is_some(),
                    replayed_node.is_some(),
                    "liveness of #{id} diverged after replay"
                );
                let (Some(direct_node), Some(replayed_node)) = (direct_node, replayed_node) else {
                    continue;
                };
                let direct_children: Vec<u32> = self
                    .direct
                    .document()
                    .get(direct_node)
                    .unwrap()
                    .child_ids()
                    .iter()
                    .map(|&child| *self.direct.document().get(child).unwrap().payload())
                    .collect();
                let replayed_children: Vec<u32> = replayed
                    .document()
                    .get(replayed_node)
                    .unwrap()
                    .child_ids()
                    .iter()
                    .map(|&child| *replayed.document().get(child).unwrap().payload())
                    .collect();
                assert_eq!(
                    direct_children, replayed_children,
                    "children of #{id} diverged after replay"
                );
            }
            assert_eq!(self.direct.page(), replayed.page());
        }
    }

    #[test]
    fn a_recorded_build_replays_into_the_same_tree() {
        let mut mirror = Mirror::new();
        let page = mirror.create_page("card", 7);
        let first = mirror.create_view(0).unwrap();
        let second = mirror.create_view(page).unwrap();
        let third = mirror.create_view(0).unwrap();
        mirror.append_element(page, first).unwrap();
        mirror.append_element(page, second).unwrap();
        mirror.append_element(first, third).unwrap();
        // Reparent: `third` moves from `first` to `second`.
        mirror.append_element(second, third).unwrap();
        mirror.assert_replay_matches();
    }

    #[test]
    fn a_recorded_subtree_drop_replays_into_the_same_tree() {
        let mut mirror = Mirror::new();
        let page = mirror.create_page("card", 0);
        let parent = mirror.create_view(0).unwrap();
        let child = mirror.create_view(0).unwrap();
        mirror.append_element(page, parent).unwrap();
        mirror.append_element(parent, child).unwrap();
        mirror.drop_element(parent).unwrap();
        // Ids keep advancing past the tombstones on both sides.
        let next = mirror.create_view(0).unwrap();
        assert_eq!(next, 4);
        mirror.append_element(page, next).unwrap();
        mirror.assert_replay_matches();
    }

    #[test]
    fn every_rejection_mirrors_the_tree_in_the_same_order() {
        let mut mirror = Mirror::new();
        let page = mirror.create_page("card", 0);
        let outer = mirror.create_view(0).unwrap();
        let inner = mirror.create_view(0).unwrap();
        mirror.append_element(page, outer).unwrap();
        mirror.append_element(outer, inner).unwrap();

        // Unknown handles, parent named before child.
        assert_eq!(
            mirror.append_element(9999, 8888).unwrap_err(),
            PapiError::UnknownElement(9999)
        );
        assert_eq!(
            mirror.append_element(page, 8888).unwrap_err(),
            PapiError::UnknownElement(8888)
        );
        // The page guard precedes the cycle check.
        assert_eq!(
            mirror.append_element(outer, page).unwrap_err(),
            PapiError::CannotReparentPage
        );
        // Cycles, including the self-append.
        assert_eq!(
            mirror.append_element(inner, outer).unwrap_err(),
            PapiError::WouldCycle {
                parent: inner,
                child: outer,
            }
        );
        assert_eq!(
            mirror.append_element(outer, outer).unwrap_err(),
            PapiError::WouldCycle {
                parent: outer,
                child: outer,
            }
        );
        // Unknown parent components on creation.
        assert_eq!(
            mirror.create_view(9999).unwrap_err(),
            PapiError::UnknownElement(9999)
        );
        // The permanent page cannot be dropped; unknown drops name the id.
        assert_eq!(
            mirror.drop_element(page).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            mirror.drop_element(4321).unwrap_err(),
            PapiError::UnknownElement(4321)
        );
        // A retired id stays unknown forever on both sides.
        mirror.drop_element(inner).unwrap();
        assert_eq!(
            mirror.drop_element(inner).unwrap_err(),
            PapiError::UnknownElement(inner)
        );
        mirror.assert_replay_matches();
    }

    #[test]
    fn rejected_calls_record_no_ops() {
        let mut recorder = ElementOpRecorder::new();
        recorder.create_page("card", 0);
        assert!(recorder.append_element(9999, 1).is_err());
        assert!(recorder.drop_element(1).is_err());
        assert!(recorder.drop_element(77).is_err());
        assert!(recorder.create_view(55).is_err());
        assert_eq!(
            recorder.take_ops(),
            [ElementOp::CreatePage {
                component_id: "card".to_owned(),
                component_css_id: 0,
            }]
        );
    }

    #[test]
    fn repeated_create_page_records_one_binding_write() {
        let mut recorder = ElementOpRecorder::new();
        assert_eq!(recorder.create_page("card", 3), 1);
        assert_eq!(recorder.create_page("other", 9), 1);
        assert_eq!(recorder.take_ops().len(), 1);
        assert!(recorder.take_ops().is_empty());
    }

    #[test]
    fn the_page_is_appendable_before_create_page() {
        // The page element pre-exists in the document, so appending under it
        // works before `__CreatePage` names it — on both sides.
        let mut mirror = Mirror::new();
        let view = mirror.create_view(0).unwrap();
        mirror.append_element(1, view).unwrap();
        mirror.assert_replay_matches();
    }

    #[test]
    fn take_ops_drains_in_flush_batches() {
        let mut recorder = ElementOpRecorder::new();
        recorder.create_page("card", 0);
        let first = recorder.create_view(0).unwrap();
        assert_eq!(recorder.take_ops().len(), 2);

        recorder.append_element(1, first).unwrap();
        let batch = recorder.take_ops();
        assert_eq!(
            batch,
            [ElementOp::AppendElement {
                parent: 1,
                child: first,
            }]
        );
    }
}
