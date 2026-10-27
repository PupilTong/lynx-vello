//! `content-visibility: auto` relevance
//! ([css-contain-2 §4.1](https://drafts.csswg.org/css-contain-2/#relevant-to-the-user)):
//! the one bit that decides whether an `auto` box skips its contents.
//!
//! It is **layout-side per-element state**, never a Stylo `ElementState` and
//! never a restyle trigger: flipping it changes what layout computes, not what
//! the cascade produces, so it must not be able to invalidate a selector match.
//! It lives in a slot-keyed side table on [`TreeArenas`](crate::tree::TreeArenas)
//! rather than beside the scroll offset in `DocumentLayoutState`, for one
//! structural reason: `LayoutTree::style` hands seldon a
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

/// How far the render in flight has determined one element.
///
/// A commit runs the determination once per build pass, and each pass may
/// move what the next one sees: a revealed box takes its real size in place
/// of its `contain-intrinsic-size`, which moves every box after it. A box an
/// earlier pass decided is therefore asked again only when the answer can
/// still go one way — from skipped to relevant — so every element's bit moves
/// at most once per commit, whatever the later layouts moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// Not determined by the render in flight.
    #[default]
    Stale,
    /// Determined skipped by the render in flight, as it already was before
    /// it: a later pass may still reveal it, once the reveals before it have
    /// moved it into the region.
    Revealable,
    /// Determined by the render in flight, and final for it: relevant, or
    /// skipped by a flip this render made.
    Final,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Entry {
    state: Relevance,
    freshness: Freshness,
}

/// The slot-keyed relevance table.
///
/// Lazily sized like `DocumentLayoutState`: an absent entry reads as
/// [`Relevance::Undetermined`], so a document that never uses
/// `content-visibility: auto` allocates nothing here.
#[derive(Debug, Default)]
pub(crate) struct RelevanceTable {
    entries: Vec<Entry>,
    /// The keys this render determined, so clearing them at the end of a
    /// render costs one walk of what this render determined rather than one
    /// walk of the document.
    fresh: Vec<usize>,
    /// Whether the last render stopped at its pass cap with a reveal still
    /// owed; the next frame tick takes it.
    owed: bool,
}

impl RelevanceTable {
    #[inline]
    pub(crate) fn state(&self, key: usize) -> Relevance {
        self.entries
            .get(key)
            .map_or(Relevance::default(), |entry| entry.state)
    }

    /// Whether the last render left a reveal owed.
    #[inline]
    pub(crate) const fn owed(&self) -> bool {
        self.owed
    }

    /// Records whether the render in flight leaves a reveal owed.
    #[inline]
    pub(crate) fn set_owed(&mut self, owed: bool) {
        self.owed = owed;
    }

    /// Takes the owed reveal, answering whether there was one.
    #[inline]
    pub(crate) fn take_owed(&mut self) -> bool {
        std::mem::take(&mut self.owed)
    }

    /// How far the render in flight has determined this element.
    #[inline]
    pub(crate) fn freshness(&self, key: usize) -> Freshness {
        self.entries
            .get(key)
            .map_or(Freshness::Stale, |entry| entry.freshness)
    }

    /// Records `state` for `key` and marks it determined for this render:
    /// [`Freshness::Revealable`] when it confirms a skip, otherwise
    /// [`Freshness::Final`].
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
        if entry.freshness == Freshness::Stale {
            self.fresh.push(key);
        }
        entry.freshness = if state.skips() && !flipped {
            Freshness::Revealable
        } else {
            Freshness::Final
        };
        flipped
    }

    /// Ends the render: every determination made during it stops being this
    /// render's, so the next one asks again.
    pub(crate) fn settle(&mut self) {
        for key in self.fresh.drain(..) {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.freshness = Freshness::Stale;
            }
        }
    }

    /// Gives `key` the relevance `from` was last determined to have, when
    /// `key` has none of its own yet; answers whether it took one.
    ///
    /// Only a relevant answer is taken: an undetermined box already skips,
    /// so a skipped one would change nothing. Freshness is left alone,
    /// because no render is in flight between two commits, which is the
    /// only time a host hands an answer over.
    pub(crate) fn inherit(&mut self, key: usize, from: usize) -> bool {
        if self.state(key) != Relevance::Undetermined || self.state(from) != Relevance::Relevant {
            return false;
        }
        if self.entries.len() <= key {
            self.entries.resize(key + 1, Entry::default());
        }
        self.entries[key].state = Relevance::Relevant;
        true
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
    use super::{Freshness, Relevance, RelevanceTable};

    #[test]
    fn an_undetermined_element_skips_its_contents() {
        let table = RelevanceTable::default();
        assert_eq!(table.state(7), Relevance::Undetermined);
        assert!(table.state(7).skips());
        assert_eq!(table.freshness(7), Freshness::Stale);
        assert!(Relevance::Skipped.skips());
        assert!(!Relevance::Relevant.skips());
        assert_eq!(Relevance::of(true), Relevance::Relevant);
        assert_eq!(Relevance::of(false), Relevance::Skipped);
    }

    #[test]
    fn only_a_relevant_answer_passes_to_an_undetermined_element() {
        let mut table = RelevanceTable::default();
        table.determine(1, Relevance::Relevant);
        table.determine(2, Relevance::Skipped);
        table.settle();
        assert!(
            !table.inherit(5, 2),
            "a skip changes nothing an undetermined box assumed"
        );
        assert!(!table.inherit(5, 9), "nor does an undetermined one");
        assert_eq!(table.state(5), Relevance::Undetermined);
        assert!(table.inherit(5, 1));
        assert_eq!(table.state(5), Relevance::Relevant);
        assert_eq!(
            table.freshness(5),
            Freshness::Stale,
            "no render is in flight"
        );
        assert!(
            !table.inherit(2, 1),
            "a determined element keeps its own answer"
        );
        assert_eq!(table.state(2), Relevance::Skipped);
    }

    #[test]
    fn determination_reports_only_real_flips_and_settles_per_render() {
        let mut table = RelevanceTable::default();
        assert!(
            !table.determine(1, Relevance::Skipped),
            "confirming what an undetermined box already assumed is not a flip"
        );
        assert_eq!(
            table.freshness(1),
            Freshness::Revealable,
            "and it still counts as determined, revealable by a later pass"
        );
        assert!(
            table.determine(3, Relevance::Relevant),
            "undetermined -> relevant flips"
        );
        assert_eq!(table.freshness(3), Freshness::Final);
        assert!(
            !table.determine(3, Relevance::Relevant),
            "re-determining the same state is not a flip"
        );
        table.settle();
        assert_eq!(
            table.freshness(3),
            Freshness::Stale,
            "settling releases the render's marks"
        );
        assert_eq!(
            table.state(3),
            Relevance::Relevant,
            "the state itself survives"
        );
        assert!(table.determine(3, Relevance::Skipped));
        assert_eq!(
            table.freshness(3),
            Freshness::Final,
            "a box this render skipped stays skipped for it"
        );
        table.reset(3);
        assert_eq!(table.state(3), Relevance::Undetermined);
        table.settle();
    }
}
