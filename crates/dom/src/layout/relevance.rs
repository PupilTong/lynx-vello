//! `content-visibility: auto` relevance
//! ([css-contain-2 §4.1](https://drafts.csswg.org/css-contain-2/#relevant-to-the-user)):
//! the one bit that decides whether an `auto` box skips its contents.
//!
//! It is **layout-side per-element state**, never a Stylo `ElementState` and
//! never a restyle trigger: flipping it changes what layout computes, not what
//! the cascade produces, so it must not be able to invalidate a selector match.
//! It lives in a slot-keyed side table on [`TreeArenas`](crate::tree::TreeArenas)
//! rather than beside the scroll offset in `DocumentLayoutState`, for one
//! structural reason: `LayoutTree::style` hands hughie a
//! [`StyleView`](crate::layout::StyleView) built from the tree arenas alone —
//! the layout state is a separately borrowed parameter the style view cannot
//! reach — so the bit has to be readable from `&Node`, which
//! [`Node::arenas`](crate::tree::node::Node::arenas) makes it.
//!
//! Three states, because "not yet asked" is not the same answer as "asked and
//! told no". The spec determines relevance in the *next rendering update*, so
//! an `auto` box that no rendering update has reached yet skips its contents:
//! that is what [`Relevance::Undetermined`] means, and it is why it reads as
//! skipped everywhere.

/// One element's relevance, as of the last rendering update that determined
/// it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Relevance {
    /// No rendering update has determined this element yet. Skips its
    /// contents — css-contain-2 §4.1 determines relevance in the next
    /// rendering update, and until then the box is not relevant.
    #[default]
    Undetermined,
    /// The element intersects the region the frame's encode window admits.
    Relevant,
    /// Determined, and not relevant: contents are skipped.
    Skipped,
}

impl Relevance {
    /// Whether an `auto` box in this state skips its contents.
    #[inline]
    pub(crate) const fn skips(self) -> bool {
        !matches!(self, Self::Relevant)
    }

    #[inline]
    pub(crate) const fn of(relevant: bool) -> Self {
        if relevant {
            Self::Relevant
        } else {
            Self::Skipped
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Entry {
    state: Relevance,
    /// Whether the render currently in flight already determined this
    /// element. A commit runs the determination once per build pass, and a
    /// node determined by an earlier pass is deliberately not re-asked by a
    /// later one — that is what makes the pass loop terminate whatever the
    /// second layout moved.
    fresh: bool,
}

/// The slot-keyed relevance table.
///
/// Lazily sized like `DocumentLayoutState`: an absent entry reads as
/// [`Relevance::Undetermined`], so a document that never uses
/// `content-visibility: auto` allocates nothing here.
#[derive(Debug, Default)]
pub(crate) struct RelevanceTable {
    entries: Vec<Entry>,
    /// The keys whose `fresh` bit is set, so clearing them at the end of a
    /// render costs one walk of what this render determined rather than one
    /// walk of the document.
    fresh: Vec<usize>,
}

impl RelevanceTable {
    #[inline]
    pub(crate) fn state(&self, key: usize) -> Relevance {
        self.entries
            .get(key)
            .map_or(Relevance::default(), |entry| entry.state)
    }

    /// Whether the render in flight has already determined this element.
    #[inline]
    pub(crate) fn is_fresh(&self, key: usize) -> bool {
        self.entries.get(key).is_some_and(|entry| entry.fresh)
    }

    /// Records `state` for `key` and marks it determined for this render.
    ///
    /// Returns whether *skipping* changed — the caller's signal that layout
    /// under this node has to be redone. Undetermined and skipped both skip,
    /// so the first rendering update of an off-screen `auto` box confirms
    /// what layout already assumed and costs no second build pass.
    pub(crate) fn determine(&mut self, key: usize, state: Relevance) -> bool {
        if self.entries.len() <= key {
            self.entries.resize(key + 1, Entry::default());
        }
        let entry = &mut self.entries[key];
        let flipped = entry.state.skips() != state.skips();
        entry.state = state;
        if !entry.fresh {
            entry.fresh = true;
            self.fresh.push(key);
        }
        flipped
    }

    /// Ends the render: every determination made during it stops being this
    /// render's, so the next one asks again.
    pub(crate) fn settle(&mut self) {
        for key in self.fresh.drain(..) {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.fresh = false;
            }
        }
    }

    /// Resets a freed slot, so the key's next occupant starts undetermined.
    ///
    /// The key may still be listed in `fresh`; clearing an already-clear bit
    /// is a no-op, and no node is freed in the middle of a render, so the
    /// stale listing can never un-freshen a live determination.
    pub(crate) fn reset(&mut self, key: usize) {
        if let Some(entry) = self.entries.get_mut(key) {
            *entry = Entry::default();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Relevance, RelevanceTable};

    #[test]
    fn an_undetermined_element_skips_its_contents() {
        let table = RelevanceTable::default();
        assert_eq!(table.state(7), Relevance::Undetermined);
        assert!(table.state(7).skips());
        assert!(!table.is_fresh(7));
        assert!(Relevance::Skipped.skips());
        assert!(!Relevance::Relevant.skips());
        assert_eq!(Relevance::of(true), Relevance::Relevant);
        assert_eq!(Relevance::of(false), Relevance::Skipped);
    }

    #[test]
    fn determination_reports_only_real_flips_and_settles_per_render() {
        let mut table = RelevanceTable::default();
        assert!(
            !table.determine(1, Relevance::Skipped),
            "confirming what an undetermined box already assumed is not a flip"
        );
        assert!(table.is_fresh(1), "and it still counts as determined");
        assert!(
            table.determine(3, Relevance::Relevant),
            "undetermined -> relevant flips"
        );
        assert!(table.is_fresh(3));
        assert!(
            !table.determine(3, Relevance::Relevant),
            "re-determining the same state is not a flip"
        );
        table.settle();
        assert!(!table.is_fresh(3), "settling releases the render's marks");
        assert_eq!(
            table.state(3),
            Relevance::Relevant,
            "the state itself survives"
        );
        assert!(table.determine(3, Relevance::Skipped));
        table.reset(3);
        assert_eq!(table.state(3), Relevance::Undetermined);
        table.settle();
    }
}
