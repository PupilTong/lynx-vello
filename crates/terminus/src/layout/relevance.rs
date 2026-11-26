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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Relevance {
    #[default]
    Undetermined,
    Relevant,
    Skipped,
}

impl Relevance {
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
pub(crate) enum Freshness {
    #[default]
    Stale,
    Revealable,
    Final,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Entry {
    state: Relevance,
    freshness: Freshness,
}

#[derive(Debug, Default)]
pub(crate) struct RelevanceTable {
    entries: Vec<Entry>,
    fresh: Vec<usize>,
    owed: bool,
}

impl RelevanceTable {
    #[inline]
    pub(crate) fn state(&self, key: usize) -> Relevance {
        self.entries
            .get(key)
            .map_or(Relevance::default(), |entry| entry.state)
    }

    #[inline]
    pub(crate) const fn owed(&self) -> bool {
        self.owed
    }

    #[inline]
    pub(crate) fn set_owed(&mut self, owed: bool) {
        self.owed = owed;
    }

    #[inline]
    pub(crate) fn take_owed(&mut self) -> bool {
        std::mem::take(&mut self.owed)
    }

    #[inline]
    pub(crate) fn freshness(&self, key: usize) -> Freshness {
        self.entries
            .get(key)
            .map_or(Freshness::Stale, |entry| entry.freshness)
    }

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

    pub(crate) fn settle(&mut self) {
        for key in self.fresh.drain(..) {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.freshness = Freshness::Stale;
            }
        }
    }

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
