//! `mask-image` painting for the walker's alpha-mask sandwich
//! (css-masking-1).
//!
//! The walker isolates the group, calls [`paint`] to draw the mask pattern
//! first, then pushes a `Compose::SrcIn` layer for the content — on pop the
//! content keeps the mask's alpha (`mask-mode` resolves to alpha for CSS
//! images/gradients under `match-source`).
//!
//! Spec sketch:
//! - Resolve each `mask-image` layer exactly like a background layer (origin from `mask-origin`,
//!   position/size/repeat/clip from the `mask-*` longhands in the SVG style struct), reusing
//!   `background`'s pattern machinery ([`background::paint_pattern_layer`]). Recorded v1 limit:
//!   paint the **first** non-`None` layer only (multi-layer `mask-composite`
//!   add/subtract/intersect/exclude needs nested compositing — follow-up).
//! - Geometry resolves against the establishing element's boxes (`fragment`), in layer-local space.

use stylo::computed_values::mask_origin::single_value::T as MaskOrigin;
use stylo::properties::ComputedValues;
use stylo::values::computed::{BackgroundClip, Image};

use crate::ImageStore;
use crate::paint::BoxFragment;
use crate::paint::background::{self, BoxLevel, PatternLayer};
use crate::vello::Scene;

pub(crate) fn has_mask(style: &ComputedValues) -> bool {
    style
        .get_svg()
        .mask_image
        .0
        .iter()
        .any(|image| !matches!(image, Image::None))
}

pub(crate) fn paint(
    scene: &mut Scene,
    style: &ComputedValues,
    fragment: &BoxFragment,
    images: &ImageStore,
) {
    let svg = style.get_svg();
    let layers = svg.mask_image.0.as_slice();
    let Some(index) = layers
        .iter()
        .position(|image| !matches!(image, Image::None))
    else {
        return;
    };
    let origin = match *background::cycled(&svg.mask_origin.0, index) {
        MaskOrigin::BorderBox => BoxLevel::Border,
        MaskOrigin::PaddingBox => BoxLevel::Padding,
        MaskOrigin::ContentBox => BoxLevel::Content,
    };
    let clip = match *background::cycled(&svg.mask_clip.0, index) {
        BackgroundClip::PaddingBox => BoxLevel::Padding,
        BackgroundClip::ContentBox => BoxLevel::Content,
        BackgroundClip::BorderBox | BackgroundClip::Text | BackgroundClip::BorderArea => {
            BoxLevel::Border
        }
    };
    let layer = PatternLayer {
        image: &layers[index],
        position_x: background::cycled(&svg.mask_position_x.0, index),
        position_y: background::cycled(&svg.mask_position_y.0, index),
        size: background::cycled(&svg.mask_size.0, index),
        repeat: background::cycled(&svg.mask_repeat.0, index),
        origin: background::level_rect(fragment, origin),
        clip: background::level_shape(fragment, clip),
    };
    background::paint_pattern_layer(scene, style, fragment, images, &layer);
}
