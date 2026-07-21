//! CSS containment: effective-containment derivation.
//!
//! `contain` and `content-visibility` are enabled in the vendored stylo fork
//! as a deliberate lynx-vello extension beyond Lynx's own property set (Lynx
//! has no containment property at all; see the fork's `lynx_properties.txt`
//! seed notes and `docs/style-assumptions.md`). stylo parses and cascades
//! them, but — unlike its gecko build — the servo build stores only the
//! **raw** `contain` value. It does not compute the *effective* containment,
//! i.e. the raw value folded with the containment that `content-visibility`
//! implies.
//!
//! [`effective_containment`] performs that fold, mirroring gecko's
//! `StyleAdjuster::adjust_for_contain` (`vendor/stylo/style/style_adjuster.rs`).
//! `container-type` is intentionally excluded: container queries (contain-3)
//! are out of scope and `container-type`/`container-name` stay disabled.
//!
//! The layout engine keeps its **own** copy of this fold for its box-layout
//! consumers — `neutron_star::style::effective_containment` (over decomposed
//! stylo values, since it has no `ComputedValues` adapter yet) — mirroring the
//! same rationale. The two copies are deliberately independent: there is no
//! crate dependency between `w3c-dom` and `neutron-star`.

use stylo::properties::ComputedValues;
pub use stylo::values::computed::{Contain, ContentVisibility};

/// The **effective** containment for `style`: the raw `contain` value folded
/// with the containment implied by `content-visibility`.
///
/// - `content-visibility: visible` — no addition; the raw `contain` stands.
/// - `content-visibility: auto` — adds `layout | paint | style`, plus `size` when
///   `skipped_contents` is true (the element's content is not relevant to the user and is being
///   skipped). v1 does not track relevance itself, so the host supplies `skipped_contents`; see the
///   scope note in `docs/style-assumptions.md`.
/// - `content-visibility: hidden` — adds `layout | paint | size | style` (its content is always
///   skipped).
///
/// The returned [`Contain`]'s **effect** bits (`LAYOUT` / `STYLE` / `PAINT` /
/// `SIZE`, and the `INLINE_SIZE` / `BLOCK_SIZE` sub-bits `SIZE` carries) are
/// authoritative and are what consumers should query via
/// [`Contain::contains`]. The value may also carry the composite *marker* bits
/// (`Contain::CONTENT` = `1 << 6`, `Contain::STRICT` = `1 << 7`) when the
/// author wrote `contain: content` / `contain: strict`; because those markers
/// are private to each composite, `result.contains(Contain::STRICT)` is **not**
/// a reliable "is size+layout contained" test — query the effect bits instead.
#[must_use]
pub fn effective_containment(style: &ComputedValues, skipped_contents: bool) -> Contain {
    let mut contain = style.clone_contain();
    match style.clone_content_visibility() {
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
