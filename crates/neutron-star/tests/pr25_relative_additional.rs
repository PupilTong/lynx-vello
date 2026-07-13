//! Relative-related PR #25 coverage outside the dedicated direct suite.

mod support;

use neutron_star::compute::round_layout;
use neutron_star::prelude::*;
use neutron_star::style::{Dimension, LengthPercentageAuto};
use support::{TestStyle, TestTree, definite_layout, fixed_leaf, relative_container};

fn lower_host_sticky_insets(
    insets: Edges<LengthPercentageAuto>,
    containing_size: Size<f32>,
) -> Edges<Option<f32>> {
    fn lower(value: LengthPercentageAuto, basis: f32) -> Option<f32> {
        match value {
            LengthPercentageAuto::Length(value) => Some(value),
            LengthPercentageAuto::Percent(fraction) => Some(fraction * basis),
            LengthPercentageAuto::Calc(_) => {
                panic!("the PR #25 sticky boundary fixtures contain no calc()")
            }
            LengthPercentageAuto::Auto => None,
        }
    }

    Edges {
        left: lower(insets.left, containing_size.width),
        right: lower(insets.right, containing_size.width),
        top: lower(insets.top, containing_size.height),
        bottom: lower(insets.bottom, containing_size.height),
    }
}

fn assert_relative_sticky_boundary(
    authored: Edges<LengthPercentageAuto>,
    expected: Edges<Option<f32>>,
) {
    let mut tree = TestTree::default();
    let sticky = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(100.0), Dimension::Length(40.0)),
            ..TestStyle::default()
        },
        &[sticky],
    );

    // Sticky is a host post-pass. The Relative algorithm receives the normal
    // in-flow box, while the host retains and resolves these authored insets.
    definite_layout(&mut tree, root, 100.0, 40.0);
    assert_eq!(tree.layout(sticky).location, Point::ZERO);
    assert_eq!(
        lower_host_sticky_insets(authored, Size::new(100.0, 40.0)),
        expected
    );
}

#[test]
fn relative_sticky_child_percent_insets_resolve_against_container_constraints() {
    assert_relative_sticky_boundary(
        Edges {
            left: LengthPercentageAuto::Percent(0.10),
            right: LengthPercentageAuto::Auto,
            top: LengthPercentageAuto::Percent(0.25),
            bottom: LengthPercentageAuto::Auto,
        },
        Edges {
            left: Some(10.0),
            right: None,
            top: Some(10.0),
            bottom: None,
        },
    );
}

#[test]
fn relative_sticky_child_end_percent_insets_resolve_against_container_constraints() {
    assert_relative_sticky_boundary(
        Edges {
            left: LengthPercentageAuto::Auto,
            right: LengthPercentageAuto::Percent(0.20),
            top: LengthPercentageAuto::Auto,
            bottom: LengthPercentageAuto::Percent(0.50),
        },
        Edges {
            left: None,
            right: Some(20.0),
            top: None,
            bottom: Some(20.0),
        },
    );
}

#[test]
fn relative_fractional_geometry_rounds_only_in_the_device_pixel_pass() {
    let mut tree = TestTree::default();
    let anchor = fixed_leaf(&mut tree, 31.25, 17.75);
    tree.source_node_mut(anchor).style.margin.left = LengthPercentageAuto::Length(1.25);
    tree.source_node_mut(anchor).style.margin.top = LengthPercentageAuto::Length(1.75);
    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(158.25), Dimension::Length(96.75)),
            ..TestStyle::default()
        },
        &[anchor],
    );

    let output = definite_layout(&mut tree, root, 158.25, 96.75);
    let mut root_layout = Layout::default();
    root_layout.size = output.size;
    tree.session.set_unrounded_layout(root, &root_layout);
    assert_eq!(tree.layout(anchor).location, Point::new(1.25, 1.75));

    round_layout(&tree.source, &mut tree.session, root, 2.0);

    assert_eq!(tree.final_layout(root).size, Size::new(158.5, 97.0));
    assert_eq!(tree.final_layout(anchor).location, Point::new(1.5, 2.0));
    assert_eq!(tree.final_layout(anchor).size, Size::new(31.0, 17.5));
}
