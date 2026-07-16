//! Unit tests for [`effective_containment`]: folding the raw `contain` value
//! with the containment `content-visibility` implies, mirroring gecko's
//! `StyleAdjuster::adjust_for_contain`.
//!
//! `contain` / `content-visibility` are a deliberate lynx-vello extension
//! enabled in the vendored stylo fork (Lynx has no containment property).

mod common;

use common::Doc;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use w3c_dom::{Contain, effective_containment};

/// Compute a node's style with the given inline declarations.
fn styled(inline: &str) -> Arc<ComputedValues> {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    if !inline.is_empty() {
        doc.set_inline(el, inline);
    }
    doc.flush();
    doc.style(el)
}

/// The four containment effect bits, individually (never the composite
/// `CONTENT` / `STRICT` markers, which carry extra private bits).
const EFFECT_BITS: [(Contain, &str); 4] = [
    (Contain::LAYOUT, "LAYOUT"),
    (Contain::PAINT, "PAINT"),
    (Contain::SIZE, "SIZE"),
    (Contain::STYLE, "STYLE"),
];

/// Assert `actual` and `expected` agree on each containment effect bit,
/// comparing bit by bit so the composite marker bits never enter the picture.
#[track_caller]
fn assert_effect(actual: Contain, expected: Contain) {
    for (bit, name) in EFFECT_BITS {
        assert_eq!(actual.contains(bit), expected.contains(bit), "{name}");
    }
}

#[test]
fn no_containment_is_none() {
    let style = styled("");
    assert_effect(effective_containment(&style, false), Contain::empty());
}

#[test]
fn content_visibility_auto_adds_layout_paint_style() {
    let style = styled("content-visibility: auto");
    // Not skipped: no size containment yet.
    assert_effect(
        effective_containment(&style, false),
        Contain::LAYOUT | Contain::PAINT | Contain::STYLE,
    );
    // Skipped (host reports the content as not relevant): size is added.
    assert_effect(
        effective_containment(&style, true),
        Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
    );
}

#[test]
fn content_visibility_hidden_adds_all_four() {
    let style = styled("content-visibility: hidden");
    // Hidden always skips its content, so `skipped` does not change the result.
    for skipped in [false, true] {
        assert_effect(
            effective_containment(&style, skipped),
            Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
        );
    }
}

#[test]
fn raw_contain_layout_is_layout_only() {
    let style = styled("contain: layout");
    assert_effect(effective_containment(&style, false), Contain::LAYOUT);
}

#[test]
fn raw_contain_size_is_size_only() {
    let style = styled("contain: size");
    assert_effect(effective_containment(&style, false), Contain::SIZE);
}

#[test]
fn raw_contain_content_is_layout_style_paint() {
    let style = styled("contain: content");
    assert_effect(
        effective_containment(&style, false),
        Contain::LAYOUT | Contain::PAINT | Contain::STYLE,
    );
}

#[test]
fn raw_contain_strict_is_all_four_via_effect_bits() {
    let style = styled("contain: strict");
    let effective = effective_containment(&style, false);
    assert_effect(
        effective,
        Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
    );
    // Query by the composed effect bits, which is reliable...
    assert!(effective.contains(Contain::LAYOUT | Contain::STYLE | Contain::PAINT | Contain::SIZE));
    // ...never by the composite marker (this is the trap the helper avoids):
    // `STRICT` carries a private `1 << 7` bit that `CONTENT` (`1 << 6`) lacks,
    // so `STRICT.contains(CONTENT)` is false even though strict ⊃ content.
    assert!(!Contain::STRICT.contains(Contain::CONTENT));
}

#[test]
fn content_visibility_combines_with_raw_contain() {
    // Raw `contain: paint` plus `content-visibility: hidden`: the union carries
    // every effect bit hidden implies, on top of the author's paint.
    let style = styled("contain: paint; content-visibility: hidden");
    assert_effect(
        effective_containment(&style, false),
        Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
    );
}
