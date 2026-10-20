//! Containment-bounded, damage-driven cache invalidation.

use crate::style::{Contain, CoreStyle};
use crate::tree::LayoutTree;

#[must_use]
pub fn is_relayout_boundary<S: CoreStyle>(style: &S) -> bool {
    let containment = style.containment();
    containment.contains(Contain::SIZE) && containment.contains(Contain::LAYOUT)
}

pub fn invalidate_for_relayout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    ancestors: impl Iterator<Item = T::NodeId>,
) -> T::NodeId {
    tree.clear_layout_cache(state, node);
    let mut root = node;
    for ancestor in ancestors {
        tree.clear_layout_cache(state, ancestor);
        root = ancestor;
        if is_relayout_boundary(&tree.style(ancestor)) {
            return ancestor;
        }
    }
    root
}
