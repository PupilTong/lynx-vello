//! Restyle damage — what a style change means for downstream layout/paint.

use stylo::servo::restyle_damage::ServoRestyleDamage;

#[cfg(feature = "style-test-utils")]
use crate::document::NodeId;

/// The restyle damage produced for one node by a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StyleDamage(ServoRestyleDamage);

impl StyleDamage {
    #[must_use]
    pub(crate) fn needs_relayout(self) -> bool {
        self.0.contains(ServoRestyleDamage::RELAYOUT)
    }

    #[must_use]
    #[cfg(feature = "style-test-utils")]
    fn needs_overflow_recalculation(self) -> bool {
        self.0.contains(ServoRestyleDamage::RECALCULATE_OVERFLOW)
    }

    #[must_use]
    #[cfg(feature = "style-test-utils")]
    fn needs_stacking_context_rebuild(self) -> bool {
        self.0
            .contains(ServoRestyleDamage::REBUILD_STACKING_CONTEXT)
    }

    #[must_use]
    #[cfg(feature = "style-test-utils")]
    fn needs_repaint(self) -> bool {
        self.0.contains(ServoRestyleDamage::REPAINT)
    }

    #[must_use]
    pub(crate) fn requires_reconstruction(self) -> bool {
        self.0.bits() == u16::MAX
    }
}

impl From<ServoRestyleDamage> for StyleDamage {
    fn from(damage: ServoRestyleDamage) -> Self {
        Self(damage)
    }
}

/// Test-only view of one node's harvested restyle damage.
#[cfg(feature = "style-test-utils")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleDamageForTesting(StyleDamage);

#[cfg(feature = "style-test-utils")]
impl StyleDamageForTesting {
    pub(crate) const fn new(damage: StyleDamage) -> Self {
        Self(damage)
    }

    #[must_use]
    pub fn needs_relayout(self) -> bool {
        self.0.needs_relayout()
    }

    #[must_use]
    pub fn needs_overflow_recalculation(self) -> bool {
        self.0.needs_overflow_recalculation()
    }

    #[must_use]
    pub fn needs_stacking_context_rebuild(self) -> bool {
        self.0.needs_stacking_context_rebuild()
    }

    #[must_use]
    pub fn needs_repaint(self) -> bool {
        self.0.needs_repaint()
    }
}

/// Test-only style damage record.
#[cfg(feature = "style-test-utils")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleDamageEntryForTesting {
    pub node_id: NodeId,
    pub damage: StyleDamageForTesting,
}

/// Test-only result of forcing a standalone style flush.
#[cfg(feature = "style-test-utils")]
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct StyleFlushSummaryForTesting {
    pub damage: Vec<StyleDamageEntryForTesting>,
}
