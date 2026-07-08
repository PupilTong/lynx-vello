//! Element pseudo-class state.
//!
//! A tiny hand-rolled bit set (no `bitflags` dependency) tracking the dynamic
//! pseudo-class flags Lynx supports on an element: `:hover`, `:active`, and
//! `:focus`.
//!
//! [`PseudoState`] is the crate's public API type (it keeps `set_pseudo_state`
//! free of any stylo type in its signature); internally each [`Node`] stores
//! the equivalent stylo [`ElementState`](stylo_dom::ElementState) so
//! `selectors::Element::match_non_ts_pseudo_class` can test it directly against
//! `NonTSPseudoClass::state_flag()`. [`PseudoState::to_element_state`] is the
//! single bridge between the two.
//!
//! [`Node`]: crate::Node

use stylo_dom::ElementState;

/// The set of active dynamic pseudo-classes on an element.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct PseudoState(u8);

impl PseudoState {
    /// `:hover`.
    pub const HOVER: Self = Self(1 << 0);
    /// `:active`.
    pub const ACTIVE: Self = Self(1 << 1);
    /// `:focus`.
    pub const FOCUS: Self = Self(1 << 2);

    /// An empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether no flags are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether all of `other`'s flags are set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Set `other`'s flags.
    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Clear `other`'s flags.
    pub const fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Set or clear `other`'s flags depending on `on`.
    pub const fn set(&mut self, other: Self, on: bool) {
        if on {
            self.insert(other);
        } else {
            self.remove(other);
        }
    }

    /// Map this set to the equivalent stylo [`ElementState`].
    ///
    /// This is the only place `PseudoState` and stylo's state bits are bridged;
    /// `selectors::Element::match_non_ts_pseudo_class` matches the resulting
    /// [`ElementState`] against `NonTSPseudoClass::state_flag()`.
    #[must_use]
    pub fn to_element_state(self) -> ElementState {
        let mut state = ElementState::empty();
        state.set(ElementState::HOVER, self.contains(Self::HOVER));
        state.set(ElementState::ACTIVE, self.contains(Self::ACTIVE));
        state.set(ElementState::FOCUS, self.contains(Self::FOCUS));
        state
    }

    /// Recover a [`PseudoState`] from a stylo [`ElementState`] (the inverse of
    /// [`to_element_state`](Self::to_element_state), keeping only the three bits
    /// Lynx tracks).
    #[must_use]
    pub fn from_element_state(state: ElementState) -> Self {
        let mut out = Self::empty();
        out.set(Self::HOVER, state.contains(ElementState::HOVER));
        out.set(Self::ACTIVE, state.contains(ElementState::ACTIVE));
        out.set(Self::FOCUS, state.contains(ElementState::FOCUS));
        out
    }
}

impl std::fmt::Debug for PseudoState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PseudoState(")?;
        let mut first = true;
        for (flag, name) in [
            (Self::HOVER, "HOVER"),
            (Self::ACTIVE, "ACTIVE"),
            (Self::FOCUS, "FOCUS"),
        ] {
            if self.contains(flag) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        if first {
            f.write_str("empty")?;
        }
        f.write_str(")")
    }
}
