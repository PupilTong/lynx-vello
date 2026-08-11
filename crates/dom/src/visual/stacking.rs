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

use hughie::style::containment::effective_containment;
use hughie::style::{Contain, PositionProperty};
use stylo::properties::ComputedValues;
use stylo::values::computed::Image;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::WillChangeBits;

use crate::layout::skips_contents;

pub(crate) fn z_index_applies(position: PositionProperty, is_item: bool) -> bool {
    position != PositionProperty::Static || is_item
}

pub(crate) fn establishes_stacking_context(style: &ComputedValues, z_applies: bool) -> bool {
    let position = style.clone_position();
    if matches!(position, PositionProperty::Fixed | PositionProperty::Sticky) {
        return true;
    }
    if z_applies && !style.clone_z_index().is_auto() {
        return true;
    }
    let box_style = style.get_box();
    if box_style.has_transform_or_perspective() {
        return true;
    }
    if style.guarantees_stacking_context() {
        return true;
    }
    if !style.get_effects().filter.0.is_empty() {
        return true;
    }
    if !matches!(box_style.offset_path, OffsetPath::None) {
        return true;
    }
    if style
        .get_svg()
        .mask_image
        .0
        .iter()
        .any(|image| !matches!(image, Image::None))
    {
        return true;
    }
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
    effective_containment(
        style.clone_contain(),
        style.clone_content_visibility(),
        skips_contents(style),
    )
    .intersects(Contain::LAYOUT | Contain::PAINT)
}

pub(crate) fn needs_group_rendering(style: &ComputedValues) -> bool {
    use stylo::computed_values::isolation::T as Isolation;
    use stylo::computed_values::mix_blend_mode::T as MixBlendMode;
    use stylo::values::computed::basic_shape::ClipPath;

    let effects = style.get_effects();
    effects.opacity < 1.0
        || !effects.filter.0.is_empty()
        || effects.mix_blend_mode != MixBlendMode::Normal
        || style.get_svg().clip_path != ClipPath::None
        || style.get_box().isolation == Isolation::Isolate
        || style
            .get_svg()
            .mask_image
            .0
            .iter()
            .any(|image| !matches!(image, Image::None))
}

pub(crate) fn stack_level(style: &ComputedValues, z_applies: bool) -> i32 {
    if z_applies {
        style.clone_z_index().integer_or(0)
    } else {
        0
    }
}
