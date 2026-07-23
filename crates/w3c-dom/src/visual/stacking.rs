//! The stacking-context predicate and stack levels.
//!
//! This is the full CSS trigger set (CSS2 §9.9 + css-position-3 +
//! css-transforms-2 + filter-effects + css-masking + compositing +
//! css-will-change + css-contain), not Lynx's reduced one — the recorded
//! z-index deviation ruling (docs/tracking/deviations.md) mandates the real
//! per-context algorithm. Triggers whose properties are storage-only in the
//! fork (`isolation`, `mix-blend-mode`, individual transforms,
//! `transform-style`) are still read so they go live on a grammar rebase;
//! they are unreachable from author CSS today and therefore untestable
//! through the cascade.

use neutron_star::style::{Contain, PositionProperty};
use stylo::properties::ComputedValues;
use stylo::values::computed::Image;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::WillChangeBits;

use crate::contain::effective_containment;
use crate::layout::skips_contents;

/// Whether `z-index` applies: positioned boxes plus flex/grid items
/// (css-flexbox-1 §4.3, css-grid-1 §10.1). `is_item` is precomputed from the
/// DOM parent's display mode by the builder.
pub(crate) fn z_index_applies(position: PositionProperty, is_item: bool) -> bool {
    position != PositionProperty::Static || is_item
}

/// The full stacking-context predicate for a non-root element. The root
/// element always establishes the initial stacking context and never
/// consults this.
pub(crate) fn establishes_stacking_context(style: &ComputedValues, z_applies: bool) -> bool {
    let position = style.clone_position();
    // Fixed and sticky boxes always establish stacking contexts
    // (css-position-3), positioned boxes only with a non-auto z-index.
    if matches!(position, PositionProperty::Fixed | PositionProperty::Sticky) {
        return true;
    }
    if z_applies && !style.clone_z_index().is_auto() {
        return true;
    }
    let box_style = style.get_box();
    // transform list, individual rotate/translate/scale, perspective, and
    // transform-style: preserve-3d (the fork's own damage helper).
    if box_style.has_transform_or_perspective() {
        return true;
    }
    // opacity < 1, mix-blend-mode, clip-path, isolation: isolate.
    if style.guarantees_stacking_context() {
        return true;
    }
    if !style.get_effects().filter.0.is_empty() {
        return true;
    }
    if !matches!(box_style.offset_path, OffsetPath::None) {
        return true;
    }
    // A mask "creates a stacking context the same way that CSS opacity does"
    // (css-masking-1). Authorable in the fork grammar, though parse-gated
    // behind servo's layout.unimplemented pref today.
    if style
        .get_svg()
        .mask_image
        .0
        .iter()
        .any(|image| !matches!(image, Image::None))
    {
        return true;
    }
    // will-change induces whatever a non-initial value of the named property
    // would induce; Z_INDEX only where z-index applies at all.
    let will_change = box_style.will_change.bits;
    if will_change.intersects(
        WillChangeBits::STACKING_CONTEXT_UNCONDITIONAL
            | WillChangeBits::TRANSFORM
            | WillChangeBits::OPACITY
            | WillChangeBits::PERSPECTIVE
            | WillChangeBits::CONTAIN
            | WillChangeBits::POSITION,
    ) {
        return true;
    }
    if z_applies && will_change.intersects(WillChangeBits::Z_INDEX) {
        return true;
    }
    effective_containment(style, skips_contents(style)).intersects(Contain::LAYOUT | Contain::PAINT)
}

/// The stack level a member sorts by within its parent stacking context:
/// the z-index integer where z-index applies, otherwise 0 (`auto` counts
/// as 0 for ordering; whether it also forms a context is the predicate's
/// business, not this function's).
pub(crate) fn stack_level(style: &ComputedValues, z_applies: bool) -> i32 {
    if z_applies {
        style.clone_z_index().integer_or(0)
    } else {
        0
    }
}
