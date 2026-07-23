//! Containment-bounded, damage-driven cache invalidation.

use crate::style::{Contain, CoreStyle};
use crate::tree::LayoutNode;

#[must_use]
pub fn is_relayout_boundary<S: CoreStyle>(style: &S) -> bool {
    let containment = style.containment();
    containment.contains(Contain::SIZE) && containment.contains(Contain::LAYOUT)
}

pub fn invalidate_for_relayout<N: LayoutNode>(node: N, ancestors: impl Iterator<Item = N>) -> N {
    node.clear_layout_cache();
    let mut root = node;
    for ancestor in ancestors {
        ancestor.clear_layout_cache();
        root = ancestor;
        if is_relayout_boundary(&ancestor.style()) {
            return ancestor;
        }
    }
    root
}
