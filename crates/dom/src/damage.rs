//! Restyle damage — what a style change means for downstream layout/paint.

use stylo::servo::restyle_damage::ServoRestyleDamage;

use crate::document::NodeId;

/// The restyle damage produced for one node by a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleDamage(ServoRestyleDamage);

impl StyleDamage {
    #[must_use]
    pub fn bits(self) -> u16 {
        self.0.bits()
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn needs_relayout(self) -> bool {
        self.0.contains(ServoRestyleDamage::RELAYOUT)
    }

    #[must_use]
    pub fn needs_overflow_recalculation(self) -> bool {
        self.0.contains(ServoRestyleDamage::RECALCULATE_OVERFLOW)
    }

    #[must_use]
    pub fn needs_stacking_context_rebuild(self) -> bool {
        self.0
            .contains(ServoRestyleDamage::REBUILD_STACKING_CONTEXT)
    }

    #[must_use]
    pub fn needs_repaint(self) -> bool {
        self.0.contains(ServoRestyleDamage::REPAINT)
    }

    #[must_use]
    pub fn requires_reconstruction(self) -> bool {
        self.0.bits() == u16::MAX
    }
}

impl From<ServoRestyleDamage> for StyleDamage {
    fn from(damage: ServoRestyleDamage) -> Self {
        Self(damage)
    }
}

impl From<StyleDamage> for ServoRestyleDamage {
    fn from(damage: StyleDamage) -> Self {
        damage.0
    }
}

/// The result of a style flush: the per-node damage it produced and whether a
/// traversal actually ran.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct FlushSummary {
    pub damage: Vec<StyleDamageEntry>,
    pub status: FlushStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlushStatus {
    #[default]
    Skipped,
    Traversed,
}

/// The style damage produced for one document node during a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleDamageEntry {
    pub node_id: NodeId,
    pub damage: StyleDamage,
}

impl FlushSummary {
    #[must_use]
    pub fn has_damage(&self) -> bool {
        !self.damage.is_empty()
    }
}
