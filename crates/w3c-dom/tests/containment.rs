//! Unit tests for [`effective_containment`]: folding the raw `contain` value
//! with the containment `content-visibility` implies, mirroring gecko's
//! `StyleAdjuster::adjust_for_contain`.

mod common;

use common::Doc;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use w3c_dom::{Contain, effective_containment};

fn styled(inline: &str) -> Arc<ComputedValues> {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    if !inline.is_empty() {
        doc.set_inline(el, inline);
    }
    doc.flush();
    doc.style(el)
}

const EFFECT_BITS: [(Contain, &str); 4] = [
    (Contain::LAYOUT, "LAYOUT"),
    (Contain::PAINT, "PAINT"),
    (Contain::SIZE, "SIZE"),
    (Contain::STYLE, "STYLE"),
];

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
    assert_effect(
        effective_containment(&style, false),
        Contain::LAYOUT | Contain::PAINT | Contain::STYLE,
    );
    assert_effect(
        effective_containment(&style, true),
        Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
    );
}

#[test]
fn content_visibility_hidden_adds_all_four() {
    let style = styled("content-visibility: hidden");
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
    assert!(effective.contains(Contain::LAYOUT | Contain::STYLE | Contain::PAINT | Contain::SIZE));
    assert!(!Contain::STRICT.contains(Contain::CONTENT));
}

#[test]
fn content_visibility_combines_with_raw_contain() {
    let style = styled("contain: paint; content-visibility: hidden");
    assert_effect(
        effective_containment(&style, false),
        Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE,
    );
}
