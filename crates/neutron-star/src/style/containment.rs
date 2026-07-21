//! CSS containment ([`css-contain-2`]): the layout-relevant projection of the
//! `contain` property plus `contain-intrinsic-size`, in the stylo fork's own
//! computed-value vocabulary.
//!
//! The engine reads containment through [`CoreStyle`]:
//! [`containment`](crate::style::CoreStyle::containment) returns the stylo
//! [`Contain`] bit set, and
//! [`contain_intrinsic_width`](crate::style::CoreStyle::contain_intrinsic_width)/
//! [`contain_intrinsic_height`](crate::style::CoreStyle::contain_intrinsic_height)
//! return stylo [`ContainIntrinsicSize`]. Hosts report the **effective**
//! containment (the raw `contain` folded with what `content-visibility`
//! implies) via [`effective_containment`].
//!
//! # Two copies of the fold, by design
//!
//! [`effective_containment`] mirrors the fold in
//! `crates/w3c-dom/src/contain.rs` (which keeps its own copy for the
//! style-side consumers, taking a `&ComputedValues`). This crate stays generic
//! over [`CoreStyle`] and has no `ComputedValues` adapter yet, so its copy
//! takes the raw stylo values decomposed — the future
//! `impl CoreStyle for ComputedValues` will call it with `clone_contain()` /
//! `clone_content_visibility()`. There is deliberately **no crate dependency**
//! between the two; the shared rationale is the servo build storing only the
//! *raw* `contain` value while gecko computes the effective one (see
//! `StyleAdjuster::adjust_for_contain` in `vendor/stylo/style/style_adjuster.rs`).
//!
//! # Effect bits vs. marker bits — read before querying [`Contain`]
//!
//! stylo's [`Contain`] carries the four **effect** bits [`LAYOUT`](Contain::LAYOUT),
//! [`STYLE`](Contain::STYLE), [`PAINT`](Contain::PAINT), and [`SIZE`](Contain::SIZE)
//! (`SIZE` additionally implies the `INLINE_SIZE`/`BLOCK_SIZE` sub-bits). It
//! **also** carries composite *marker* bits when the author literally wrote the
//! `content` (`1 << 6`) or `strict` (`1 << 7`) keyword. Those markers are not
//! set when the equivalent effect bits arrive another way (e.g. `contain:
//! layout paint style` sets no `content` marker), so
//! `contain.contains(Contain::CONTENT)` / `Contain::STRICT` is **not** a
//! reliable "is effectively content/strict" test. Always query the individual
//! effect bits — `contain.contains(Contain::LAYOUT)`, `contain.contains(Contain::SIZE)`
//! — as this module and [`crate::invalidate`] do.
//!
//! # What v1 layout actually consumes
//!
//! Only `SIZE` and `LAYOUT` have box-layout effects in v1:
//!
//! - [`SIZE`](Contain::SIZE) makes every content-derived automatic size (`auto`, `min-content`,
//!   `max-content`, `fit-content`, and the Flexbox §4.5 automatic minimum) resolve **as if the box
//!   had no contents**, substituting [`contain-intrinsic-width`]/[`contain-intrinsic-height`].
//!   Children are still laid out; only the box's *own* size ignores them.
//! - [`LAYOUT`](Contain::LAYOUT) makes the box an independent formatting context and suppresses the
//!   container baseline it would export to a parent (see each algorithm's output construction). It
//!   **also changes scrollable overflow**: per [css-contain-2 §3.3][layout-containment] (item 3,
//!   "If the computed value of the overflow property is either visible or clip … any overflow must
//!   be treated as ink overflow"), a layout-contained box whose `overflow` is `visible` reports its
//!   own border box as its scrollable overflow — descendant overflow is *ink* overflow, excluded —
//!   whereas a scroll container keeps its interior union as its real scroll range. Each algorithm
//!   applies this at its output construction (the `own_scrollable_overflow` helper).
//!   (Independently, a scroll-container **child** traps its own scrollable overflow toward *this*
//!   box per [css-overflow-3 §3.3][scrollable] — the `accumulate_scrollable_overflow` helper.)
//!   Together with `PAINT`, `LAYOUT` also makes the box a containing block for abs/fixed
//!   descendants — a **host** contract, see [`PositionProperty`](crate::style::PositionProperty).
//! - `SIZE` **and** `LAYOUT` together (`contain: strict`, or a skipped `content-visibility` box)
//!   form a **relayout boundary** — the perf payoff, see
//!   [`is_relayout_boundary`](crate::invalidate::is_relayout_boundary).
//!
//! [layout-containment]: https://drafts.csswg.org/css-contain-2/#containment-layout
//! [scrollable]: https://drafts.csswg.org/css-overflow-3/#scrollable
//!
//! `PAINT` (clip + stacking context) and `STYLE` (counter/quote scoping — moot
//! in an engine with no `content` property, counters, or quotes) are carried
//! for fidelity and the containing-block trigger, but the render layer owns
//! paint and this engine has no style-containment consumer.
//!
//! # Deferrals (v1)
//!
//! - **Single-axis `inline-size` containment** (`contain-3`) is *ignored*: it never satisfies the
//!   full [`SIZE`](Contain::SIZE) test and never bounds relayout. Size containment always covers
//!   both physical axes here.
//! - **`contain-intrinsic-size: auto <length>`** (last-remembered size) is treated as a plain
//!   length, and **`auto none`** as `none` — the size-containment resolver collapses
//!   [`AutoLength`](ContainIntrinsicSize::AutoLength) to its length and
//!   [`AutoNone`](ContainIntrinsicSize::AutoNone) to `None`, until a rendered-size feedback path
//!   exists.
//!
//! [`css-contain-2`]: https://drafts.csswg.org/css-contain-2/
//! [`contain-intrinsic-width`]: crate::style::CoreStyle::contain_intrinsic_width
//! [`contain-intrinsic-height`]: crate::style::CoreStyle::contain_intrinsic_height

use crate::geometry::Size;
use crate::style::{Contain, ContainIntrinsicSize, ContentVisibility, CoreStyle};

/// The **effective** containment for a box: the raw `contain` value folded with
/// the containment its `content-visibility` implies.
///
/// - `content-visibility: visible` — no addition; the raw `contain` stands.
/// - `content-visibility: auto` — adds `layout | paint | style`, plus `size` when
///   `skipped_contents` is `true` (the element is not relevant to the user — e.g. off-screen — and
///   is skipping its content). v1 does not track relevance itself, so the host supplies
///   `skipped_contents`.
/// - `content-visibility: hidden` — adds `layout | paint | size | style` (its content is always
///   skipped).
///
/// Mirrors `w3c_dom::effective_containment` (which folds the same way from a
/// `&ComputedValues`); see the module docs for why the two copies exist. The
/// result's **effect** bits are authoritative — query them via
/// [`Contain::contains`], never the `CONTENT`/`STRICT` marker composites.
#[must_use]
pub fn effective_containment(
    contain: Contain,
    content_visibility: ContentVisibility,
    skipped_contents: bool,
) -> Contain {
    let mut contain = contain;
    match content_visibility {
        ContentVisibility::Visible => {}
        ContentVisibility::Auto => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::STYLE);
            if skipped_contents {
                contain.insert(Contain::SIZE);
            }
        }
        ContentVisibility::Hidden => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE);
        }
    }
    contain
}

/// The substitute content-box length a size-contained axis reports, or `None`.
///
/// [`AutoLength`](ContainIntrinsicSize::AutoLength) yields its length and
/// [`AutoNone`](ContainIntrinsicSize::AutoNone) yields `None` in v1 (no
/// last-remembered rendered size yet — see the module docs).
#[must_use]
pub(crate) fn contain_intrinsic_length(value: &ContainIntrinsicSize) -> Option<f32> {
    match value {
        ContainIntrinsicSize::None | ContainIntrinsicSize::AutoNone => None,
        ContainIntrinsicSize::Length(length) | ContainIntrinsicSize::AutoLength(length) => {
            Some(length.0.px())
        }
    }
}

/// The substitute content-box sizes for a size-contained box, or `None` when
/// the box is not size-contained.
///
/// Size containment always covers both physical axes (single-axis `inline-size`
/// is ignored in v1), so the result is a whole [`Size`]. Each axis is `None`
/// when its `contain-intrinsic-*` is `none`/`auto none`.
#[must_use]
pub(crate) fn size_containment<S: CoreStyle>(style: &S) -> Option<Size<Option<f32>>> {
    style.containment().contains(Contain::SIZE).then(|| {
        Size::new(
            contain_intrinsic_length(&style.contain_intrinsic_width()),
            contain_intrinsic_length(&style.contain_intrinsic_height()),
        )
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::{Contain, Length};
    use stylo::values::generics::NonNegative;

    use super::*;

    fn intrinsic_len(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
    }

    fn intrinsic_auto_len(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::AutoLength(NonNegative(Length::new(value)))
    }

    #[test]
    fn effective_containment_folds_content_visibility() {
        // Visible leaves the raw value untouched.
        assert_eq!(
            effective_containment(Contain::LAYOUT, ContentVisibility::Visible, false),
            Contain::LAYOUT
        );

        // Auto adds layout | paint | style; size only when skipped.
        let auto = effective_containment(Contain::empty(), ContentVisibility::Auto, false);
        assert!(auto.contains(Contain::LAYOUT));
        assert!(auto.contains(Contain::PAINT));
        assert!(auto.contains(Contain::STYLE));
        assert!(!auto.contains(Contain::SIZE));
        let auto_skipped = effective_containment(Contain::empty(), ContentVisibility::Auto, true);
        assert!(auto_skipped.contains(Contain::SIZE));
        assert!(auto_skipped.contains(Contain::LAYOUT));

        // Hidden adds all four regardless of the relevance signal.
        for skipped in [false, true] {
            let hidden =
                effective_containment(Contain::empty(), ContentVisibility::Hidden, skipped);
            assert!(hidden.contains(Contain::LAYOUT));
            assert!(hidden.contains(Contain::PAINT));
            assert!(hidden.contains(Contain::STYLE));
            assert!(hidden.contains(Contain::SIZE));
        }
    }

    #[test]
    fn effective_containment_reads_effect_bits_not_markers() {
        // `contain: strict` carries the 1<<7 marker plus every effect bit; the
        // effect-bit query is what consumers must use.
        let strict = effective_containment(Contain::STRICT, ContentVisibility::Visible, false);
        assert!(strict.contains(Contain::SIZE));
        assert!(strict.contains(Contain::LAYOUT));
        // `contain: content` is layout|paint|style — no size.
        let content = effective_containment(Contain::CONTENT, ContentVisibility::Visible, false);
        assert!(content.contains(Contain::LAYOUT));
        assert!(!content.contains(Contain::SIZE));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn contain_intrinsic_length_treats_auto_as_length_and_auto_none_as_none() {
        assert_eq!(contain_intrinsic_length(&ContainIntrinsicSize::None), None);
        assert_eq!(
            contain_intrinsic_length(&ContainIntrinsicSize::AutoNone),
            None
        );
        assert_eq!(contain_intrinsic_length(&intrinsic_len(40.0)), Some(40.0));
        assert_eq!(
            contain_intrinsic_length(&intrinsic_auto_len(30.0)),
            Some(30.0)
        );
    }

    struct SizeContained;
    impl CoreStyle for SizeContained {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::STRICT
        }
        fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
            intrinsic_len(50.0)
        }
        fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
            intrinsic_auto_len(30.0)
        }
    }

    struct LayoutOnly;
    impl CoreStyle for LayoutOnly {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::LAYOUT
        }
    }

    struct InlineSizeOnly;
    impl CoreStyle for InlineSizeOnly {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::INLINE_SIZE
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn size_containment_reports_both_axes_only_for_full_size_containment() {
        assert_eq!(
            size_containment(&SizeContained),
            Some(Size::new(Some(50.0), Some(30.0)))
        );
        // Layout containment without size is not a size-containment source.
        assert_eq!(size_containment(&LayoutOnly), None);
        // Single-axis inline-size containment is ignored in v1.
        assert_eq!(size_containment(&InlineSizeOnly), None);
    }
}
