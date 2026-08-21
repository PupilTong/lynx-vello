//! Scene equivalence: does one `vello::Scene` encode the same drawing as
//! another?
//!
//! Incremental painting is only admissible if the scene it produces is the
//! scene a full rebuild would have produced. That claim is checkable without
//! a GPU: [`vello::Scene::encoding`] exposes every stream the renderer reads,
//! so two scenes that agree on all of them dispatch identical work.
//!
//! What this module compares, and what it cannot:
//!
//! - Compared exactly: `path_tags`, `path_data`, `draw_tags`, `draw_data`, `transforms`, `styles`,
//!   `n_paths`, `n_path_segments`, `n_clips`, `n_open_clips`, `flags`, and the `resources`
//!   sub-streams `color_stops`, `glyphs`, `glyph_runs`, `normalized_coords`.
//! - Compared by length only: `resources.patches`. `vello_encoding::Patch` is reachable from
//!   `vello` only as an unnameable field type, and it derives neither `Debug` nor `PartialEq`, so
//!   its variants cannot be destructured here. The consequence is one real hole: `Patch::Image`
//!   carries the replaced/`url()` image's `peniko::ImageData`, and no other stream carries those
//!   pixels. Two scenes whose only difference is *which* decoded image a draw resolves to compare
//!   equal. Gradient ramps and glyph runs are safe — their patches index `color_stops` and
//!   `glyph_runs`, both compared — but `Patch::Ramp::extend` and every patch's `draw_data_offset`
//!   are likewise unobservable.
//! - Not compared at all: nothing else exists on `Encoding`; every field is public.
//!
//! Float comparison is bitwise, not `==`: an incremental path that reproduces a `NaN` or a `-0.0`
//! exactly must pass, and one that turns `-0.0` into `0.0` must fail. `color_stops` and
//! `peniko::Style` are the two exceptions, compared with their own `PartialEq` because their
//! interiors are private.
//!
//! The complete observer is the GPU: `flashbulb::capture_scene_sized` on both scenes with an
//! `assert_eq!` on the two pixel buffers closes the image hole, at the cost of an adapter.
//! `tests/gpu_pixels.rs` already does exactly that byte-exact pixel comparison for the atlas
//! isolation case.

use crate::vello::Scene;

/// The bit pattern of a `vello_encoding::Transform`, which cannot be named
/// through the `vello` re-export.
macro_rules! transform_bits {
    ($transform:expr) => {{
        let transform = $transform;
        [
            transform.matrix[0].to_bits(),
            transform.matrix[1].to_bits(),
            transform.matrix[2].to_bits(),
            transform.matrix[3].to_bits(),
            transform.translation[0].to_bits(),
            transform.translation[1].to_bits(),
        ]
    }};
}

/// Panics unless `a` and `b` encode the same drawing.
///
/// The panic message names the first stream and index that disagree.
#[track_caller]
pub(crate) fn assert_scenes_identical(a: &Scene, b: &Scene) {
    if let Err(difference) = compare_scenes(a, b) {
        panic!("the two scenes are not the same drawing: {difference}");
    }
}

/// Describes the first difference between two encoded scenes, or `Ok` when
/// every comparable stream agrees.
#[allow(
    clippy::too_many_lines,
    reason = "one linear pass over every public encoding stream"
)]
pub(crate) fn compare_scenes(a: &Scene, b: &Scene) -> Result<(), String> {
    let (left, right) = (a.encoding(), b.encoding());

    length("path_tags", left.path_tags.len(), right.path_tags.len())?;
    for (index, (l, r)) in left.path_tags.iter().zip(&right.path_tags).enumerate() {
        if l.0 != r.0 {
            return Err(format!("path_tags[{index}]: {:#04x} vs {:#04x}", l.0, r.0));
        }
    }

    length("path_data", left.path_data.len(), right.path_data.len())?;
    for (index, (l, r)) in left.path_data.iter().zip(&right.path_data).enumerate() {
        if l != r {
            return Err(format!("path_data[{index}]: {l:#010x} vs {r:#010x}"));
        }
    }

    length("draw_tags", left.draw_tags.len(), right.draw_tags.len())?;
    for (index, (l, r)) in left.draw_tags.iter().zip(&right.draw_tags).enumerate() {
        if l.0 != r.0 {
            return Err(format!(
                "draw_tags[{index}]: {:#010x} vs {:#010x}",
                l.0, r.0
            ));
        }
    }

    length("draw_data", left.draw_data.len(), right.draw_data.len())?;
    for (index, (l, r)) in left.draw_data.iter().zip(&right.draw_data).enumerate() {
        if l != r {
            return Err(format!("draw_data[{index}]: {l:#010x} vs {r:#010x}"));
        }
    }

    length("transforms", left.transforms.len(), right.transforms.len())?;
    for (index, (l, r)) in left.transforms.iter().zip(&right.transforms).enumerate() {
        if transform_bits!(l) != transform_bits!(r) {
            return Err(format!("transforms[{index}]: {l:?} vs {r:?}"));
        }
    }

    length("styles", left.styles.len(), right.styles.len())?;
    for (index, (l, r)) in left.styles.iter().zip(&right.styles).enumerate() {
        if l.flags_and_miter_limit != r.flags_and_miter_limit
            || l.line_width.to_bits() != r.line_width.to_bits()
        {
            return Err(format!("styles[{index}]: {l:?} vs {r:?}"));
        }
    }

    let (left_resources, right_resources) = (&left.resources, &right.resources);

    length(
        "resources.color_stops",
        left_resources.color_stops.len(),
        right_resources.color_stops.len(),
    )?;
    for (index, (l, r)) in left_resources
        .color_stops
        .iter()
        .zip(&right_resources.color_stops)
        .enumerate()
    {
        if l != r {
            return Err(format!("resources.color_stops[{index}]: {l:?} vs {r:?}"));
        }
    }

    length(
        "resources.glyphs",
        left_resources.glyphs.len(),
        right_resources.glyphs.len(),
    )?;
    for (index, (l, r)) in left_resources
        .glyphs
        .iter()
        .zip(&right_resources.glyphs)
        .enumerate()
    {
        if l.id != r.id || l.x.to_bits() != r.x.to_bits() || l.y.to_bits() != r.y.to_bits() {
            return Err(format!("resources.glyphs[{index}]: {l:?} vs {r:?}"));
        }
    }

    length(
        "resources.normalized_coords",
        left_resources.normalized_coords.len(),
        right_resources.normalized_coords.len(),
    )?;
    for (index, (l, r)) in left_resources
        .normalized_coords
        .iter()
        .zip(&right_resources.normalized_coords)
        .enumerate()
    {
        if l != r {
            return Err(format!("resources.normalized_coords[{index}]: {l} vs {r}"));
        }
    }

    length(
        "resources.glyph_runs",
        left_resources.glyph_runs.len(),
        right_resources.glyph_runs.len(),
    )?;
    for (index, (l, r)) in left_resources
        .glyph_runs
        .iter()
        .zip(&right_resources.glyph_runs)
        .enumerate()
    {
        let field = |name: &str| format!("resources.glyph_runs[{index}].{name}");
        if l.font != r.font {
            return Err(format!("{}: {:?} vs {:?}", field("font"), l.font, r.font));
        }
        if transform_bits!(&l.transform) != transform_bits!(&r.transform) {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("transform"),
                l.transform,
                r.transform
            ));
        }
        let (left_glyph, right_glyph) = (
            l.glyph_transform.map(|t| transform_bits!(&t)),
            r.glyph_transform.map(|t| transform_bits!(&t)),
        );
        if left_glyph != right_glyph {
            return Err(format!(
                "{}: {left_glyph:?} vs {right_glyph:?}",
                field("glyph_transform")
            ));
        }
        let (left_brush, right_brush) = (
            l.brush_transform.map(|t| transform_bits!(&t)),
            r.brush_transform.map(|t| transform_bits!(&t)),
        );
        if left_brush != right_brush {
            return Err(format!(
                "{}: {left_brush:?} vs {right_brush:?}",
                field("brush_transform")
            ));
        }
        if l.font_size.to_bits() != r.font_size.to_bits() {
            return Err(format!(
                "{}: {} vs {}",
                field("font_size"),
                l.font_size,
                r.font_size
            ));
        }
        let embolden = |run: &crate::vello::FontEmbolden| {
            (
                run.amount.xx.to_bits(),
                run.amount.yy.to_bits(),
                run.join,
                run.miter_limit.to_bits(),
                run.tolerance.to_bits(),
            )
        };
        if embolden(&l.font_embolden) != embolden(&r.font_embolden) {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("font_embolden"),
                l.font_embolden,
                r.font_embolden
            ));
        }
        if l.hint != r.hint {
            return Err(format!("{}: {} vs {}", field("hint"), l.hint, r.hint));
        }
        if l.normalized_coords != r.normalized_coords {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("normalized_coords"),
                l.normalized_coords,
                r.normalized_coords
            ));
        }
        if l.style != r.style {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("style"),
                l.style,
                r.style
            ));
        }
        if l.glyphs != r.glyphs {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("glyphs"),
                l.glyphs,
                r.glyphs
            ));
        }
        let (left_offsets, right_offsets) = (&l.stream_offsets, &r.stream_offsets);
        if (
            left_offsets.path_tags,
            left_offsets.path_data,
            left_offsets.draw_tags,
            left_offsets.draw_data,
            left_offsets.transforms,
            left_offsets.styles,
        ) != (
            right_offsets.path_tags,
            right_offsets.path_data,
            right_offsets.draw_tags,
            right_offsets.draw_data,
            right_offsets.transforms,
            right_offsets.styles,
        ) {
            return Err(format!(
                "{}: {:?} vs {:?}",
                field("stream_offsets"),
                l.stream_offsets,
                r.stream_offsets
            ));
        }
    }

    // `Patch` is not nameable through the `vello` re-export and derives
    // neither `Debug` nor `PartialEq`, so only the count is checkable here.
    length(
        "resources.patches",
        left_resources.patches.len(),
        right_resources.patches.len(),
    )?;

    scalar("n_paths", left.n_paths, right.n_paths)?;
    scalar(
        "n_path_segments",
        left.n_path_segments,
        right.n_path_segments,
    )?;
    scalar("n_clips", left.n_clips, right.n_clips)?;
    scalar("n_open_clips", left.n_open_clips, right.n_open_clips)?;
    scalar("flags", left.flags, right.flags)?;
    Ok(())
}

fn length(stream: &str, left: usize, right: usize) -> Result<(), String> {
    if left == right {
        return Ok(());
    }
    Err(format!("{stream} has {left} entries against {right}"))
}

fn scalar(name: &str, left: u32, right: u32) -> Result<(), String> {
    if left == right {
        return Ok(());
    }
    Err(format!("{name}: {left} vs {right}"))
}

#[cfg(test)]
mod probe {
    use super::{assert_scenes_identical, compare_scenes};
    use crate::test_common::Doc;
    use crate::vello::Scene;

    const CSS: &str = "page { display: flex; position: relative; width: 300px; height: 200px; }
        .card { display: flex; position: absolute; left: 10px; top: 10px;
                width: 120px; height: 60px; border-radius: 8px;
                border: 2px solid #cccccc; opacity: 0.8;
                background-image: linear-gradient(90deg, red, blue);
                box-shadow: 0px 2px 6px rgba(0,0,0,0.25); }
        .chip { display: flex; width: 30px; height: 12px; background-color: #3366ff; }";

    fn built() -> Scene {
        let mut doc = Doc::with_css(CSS);
        let root = doc.root;
        for _ in 0..4 {
            let card = doc.el(root, "view.card");
            doc.el(card, "view.chip");
        }
        doc.dom.render();
        doc.dom.scene().clone()
    }

    #[test]
    fn two_independent_renders_of_the_same_document_encode_the_same_scene() {
        assert_scenes_identical(&built(), &built());
    }

    #[test]
    fn a_changed_document_is_reported_as_a_difference() {
        let mut doc = Doc::with_css(CSS);
        let root = doc.root;
        doc.el(root, "view.card");
        doc.dom.render();
        let first = doc.dom.scene().clone();
        let card = doc.el(root, "view.card");
        doc.el(card, "view.chip");
        doc.dom.render();
        let second = doc.dom.scene().clone();
        assert!(compare_scenes(&first, &second).is_err());
    }
}
