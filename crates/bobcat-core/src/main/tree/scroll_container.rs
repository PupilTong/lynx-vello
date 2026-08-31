//! The `scroll-view` and `list` tags as main-thread scroll containers.
//!
//! Neither tag is a [`dom::CustomElement`] yet — there is no cell recycling,
//! no scroll-to-index, no threshold events. What is here is the part a UA
//! sheet can carry alone: which axis scrolls, which one clips, and which way
//! the subtree stacks. Their shared container defaults (border box, the
//! configured display mode) ride the same rule as `view`'s, in
//! [`super::ua_sheet`]. What neither the sheet nor this module covers yet is
//! recorded in `docs/tracking/deviations.md`.

/// Where a scroller scrolls, from `web-elements`' `scroll-view.css` and
/// `x-list.css`.
///
/// A Lynx scroller scrolls one axis and clips the other, and stacks its
/// children along the axis it scrolls — which takes two declarations, because
/// the axis lives in a different property per display mode: `flex-direction`
/// for `display: flex`, `linear-direction` for `display: linear`. That is the
/// pair web-elements drives through one `--lynx-linear-orientation` custom
/// property. Vertical is the default in both worlds, and
/// `enable-scroll="false"` leaves the box a scroll container that only script
/// can move (`overflow: hidden`) rather than one the user can drag.
///
/// The authored `clip` never survives computation here: css-overflow-3 turns a
/// `clip` axis into `hidden` when the other axis scrolls, so a scroller's cross
/// axis is a scroll container that only script can move rather than a plain
/// clip. That is what a browser makes of the same declaration in
/// `scroll-view.css`, so it is parity rather than a shortcut.
pub(super) const UA_RULES: &str = r#"
scroll-view, scroll-view[scroll-y], scroll-view[scroll-orientation="vertical"], list {
  overflow-x: clip; overflow-y: scroll;
  flex-direction: column; linear-direction: column;
}
scroll-view[scroll-x], scroll-view[scroll-orientation="horizontal"],
list[scroll-orientation="horizontal"] {
  overflow-x: scroll; overflow-y: clip;
  flex-direction: row; linear-direction: row;
}
scroll-view[scroll-y][enable-scroll="false"],
scroll-view[scroll-orientation="vertical"][enable-scroll="false"],
list[enable-scroll="false"] { overflow-y: hidden; }
scroll-view[scroll-x][enable-scroll="false"],
scroll-view[scroll-orientation="horizontal"][enable-scroll="false"],
list[scroll-orientation="horizontal"][enable-scroll="false"] { overflow-x: hidden; }
"#;

#[cfg(test)]
mod tests {
    use dom::stylo::computed_values::{flex_direction, linear_direction};
    use dom::stylo::values::computed::Overflow;

    use super::super::test_support::{child, document, element_under, overflow, style_of};

    #[test]
    fn a_scroller_scrolls_one_axis_and_clips_the_other() {
        let mut document = document();
        let vertical = [
            child(&mut document, "scroll-view", ""),
            child(&mut document, "list", ""),
        ];
        let horizontal = [
            child(&mut document, "scroll-view", ""),
            child(&mut document, "list", ""),
        ];
        document.set_attribute(horizontal[0], "scroll-x", "");
        document.set_attribute(horizontal[1], "scroll-orientation", "horizontal");
        document.layout();

        for scroller in vertical {
            let style = style_of(&document, scroller);
            assert_eq!(
                (style.clone_overflow_x(), style.clone_overflow_y()),
                (Overflow::Hidden, Overflow::Scroll),
                "the clipped axis computes to `hidden` beside a scrolling one"
            );
            assert_eq!(style.clone_flex_direction(), flex_direction::T::Column);
            assert_eq!(style.clone_linear_direction(), linear_direction::T::Column);
        }
        for scroller in horizontal {
            let style = style_of(&document, scroller);
            assert_eq!(
                (style.clone_overflow_x(), style.clone_overflow_y()),
                (Overflow::Scroll, Overflow::Hidden)
            );
            assert_eq!(style.clone_flex_direction(), flex_direction::T::Row);
            assert_eq!(style.clone_linear_direction(), linear_direction::T::Row);
        }
    }

    #[test]
    fn enable_scroll_false_leaves_a_scroller_only_script_can_move() {
        let mut document = document();
        let vertical = child(&mut document, "list", "");
        let horizontal = child(&mut document, "scroll-view", "");
        document.set_attribute(vertical, "enable-scroll", "false");
        document.set_attribute(horizontal, "scroll-x", "");
        document.set_attribute(horizontal, "enable-scroll", "false");
        document.layout();

        for scroller in [vertical, horizontal] {
            assert_eq!(
                overflow(&document, scroller),
                (Overflow::Hidden, Overflow::Hidden),
                "neither axis is `scroll` any more, so no drag reaches either"
            );
        }
    }

    /// The layout half of the UA gap: before these rules existed both tags fell
    /// to the bare Lynx initial values, so the subtree stacked on the wrong axis
    /// inside a content box.
    #[test]
    fn a_scroller_lays_its_subtree_out_along_the_axis_it_scrolls() {
        for (tag, attribute, horizontal) in [
            ("scroll-view", None, false),
            ("list", None, false),
            ("scroll-view", Some(("scroll-x", "")), true),
            ("list", Some(("scroll-orientation", "horizontal")), true),
        ] {
            let mut document = document();
            let scroller = child(
                &mut document,
                tag,
                "width: 100px; height: 100px; border: 10px solid",
            );
            if let Some((name, value)) = attribute {
                document.set_attribute(scroller, name, value);
            }
            let items: Vec<_> = (0..2)
                .map(|_| {
                    element_under(&mut document, scroller, "view", "width: 30px; height: 30px")
                })
                .collect();
            document.layout();

            let border_box = document
                .rounded_layout(scroller)
                .expect("the scroller is laid out");
            assert_eq!(
                (border_box.size.width, border_box.size.height),
                (100.0, 100.0),
                "border-box sizing puts the border inside the declared size: {tag}"
            );

            let offsets: Vec<_> = items
                .iter()
                .map(|item| {
                    let layout = document
                        .rounded_layout(*item)
                        .expect("a scroller lays its subtree out");
                    assert_eq!(
                        (layout.size.width, layout.size.height),
                        (30.0, 30.0),
                        "{tag}"
                    );
                    (layout.location.x, layout.location.y)
                })
                .collect();
            let stacked = if horizontal {
                [(10.0, 10.0), (40.0, 10.0)]
            } else {
                [(10.0, 10.0), (10.0, 40.0)]
            };
            assert_eq!(offsets, stacked, "{tag} horizontal={horizontal}");
        }
    }
}
