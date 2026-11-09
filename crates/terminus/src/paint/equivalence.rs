//! Display-list equivalence: do two [`DisplayList`]s record the same
//! drawing?
//!
//! Incremental painting is only admissible if the list it produces is the
//! list a full rebuild would have produced. That claim is checkable without
//! a GPU, because a list holds drawing semantics rather than a bit stream:
//! two lists that record the same items in the same order — equal shapes
//! (paths element by element), transforms, fill rules, paints, clip
//! isolation, layer blends and alphas, and glyph runs with their fonts,
//! sizes, glyphs and variation coordinates — rasterize to the same pixels.
//!
//! Every paint is compared by value: a gradient by its kind, stops and
//! extend; an image by its pixels' handle (`peniko::Blob` compares by id,
//! so two draws resolving to different decoded images differ) and its
//! sampler. Floats compare with `==`, so `0.0` and `-0.0` are the same and
//! a `NaN` is never equal to itself.
//!
//! The complete observer is the GPU: `prime_radiant::capture_scene_sized` on
//! both lists with an `assert_eq!` on the two pixel buffers.
//! `tests/gpu_pixels.rs` does that byte-exact pixel comparison for the atlas
//! isolation case.

use crate::paint::display_list::DisplayList;

/// Panics unless `a` and `b` record the same drawing.
///
/// The panic message names the first item that disagrees.
#[track_caller]
pub(crate) fn assert_scenes_identical(a: &DisplayList, b: &DisplayList) {
    if let Err(difference) = compare_scenes(a, b) {
        panic!("the two lists are not the same drawing: {difference}");
    }
}

/// Describes the first difference between two lists, or `Ok` when they
/// record the same drawing.
pub(crate) fn compare_scenes(a: &DisplayList, b: &DisplayList) -> Result<(), String> {
    a.compare(b)
}

#[cfg(test)]
mod probe {
    use super::{assert_scenes_identical, compare_scenes};
    use crate::paint::display_list::DisplayList;
    use crate::test_common::Doc;

    const CSS: &str = "page { display: flex; position: relative; width: 300px; height: 200px; }
        .card { display: flex; position: absolute; left: 10px; top: 10px;
                width: 120px; height: 60px; border-radius: 8px;
                border: 2px solid #cccccc; opacity: 0.8;
                background-image: linear-gradient(90deg, red, blue);
                box-shadow: 0px 2px 6px rgba(0,0,0,0.25); }
        .chip { display: flex; width: 30px; height: 12px; background-color: #3366ff; }";

    fn built() -> DisplayList {
        let mut doc = Doc::with_css(CSS);
        let root = doc.root;
        for _ in 0..4 {
            let card = doc.el(root, "view.card");
            doc.el(card, "view.chip");
        }
        doc.dom.render();
        doc.dom.scene(&crate::render::image::NoImages).clone()
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
        let first = doc.dom.scene(&crate::render::image::NoImages).clone();
        let card = doc.el(root, "view.card");
        doc.el(card, "view.chip");
        doc.dom.render();
        let second = doc.dom.scene(&crate::render::image::NoImages).clone();
        assert!(compare_scenes(&first, &second).is_err());
    }
}
