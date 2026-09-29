//! css-anchor-position-1 `anchor-size()` (§5.1), the one anchor function the
//! lynx fork parses, resolved the way §5.1.1 resolves it.
//!
//! The computed style keeps the function — `AnchorSizeFunction` on its own, or
//! `AnchorContainingCalcFunction` when it sits inside a math function — in the
//! sizes, min/max sizes, insets and margins, and resolving it is layout's job.
//! §5.1.1 makes it *resolvable* only when the box is absolutely positioned and
//! a target anchor element exists for the name; otherwise the fallback
//! applies, and without one the declaration is invalid at computed-value time.
//!
//! Every resolver in [`super::util`] therefore handles the functions on its
//! own, and handles them as unresolvable: a value that reaches a resolver still
//! carrying a function is one no absolute pass substituted, so the box is not
//! one §5.1.1 resolves for. The absolutely positioned path substitutes first
//! ([`AnchoredGeometry::resolve`], called by `absolute_layout`), asking the
//! host for each target's size through [`LayoutTree::anchor_size`].
//!
//! Invalid at computed-value time is approximated as the property's *initial*
//! value — `auto` for sizes and insets, `none` for max sizes, `0` for margins
//! — which is what it computes to for these non-inherited properties. The
//! approximation is that it is decided at layout time, per box, rather than
//! being visible in the computed value.
//!
//! Physical axes only: the fork disables `writing-mode`, so the anchor's and
//! the query box's block axis is vertical and their inline axis horizontal
//! ([`anchor_size_axis`]).

use stylo::logical_geometry::PhysicalAxis;
use stylo::values::computed::{
    Inset, Length, LengthPercentage, Margin, MaxSize, Size as StyleSize, ToComputedValue,
};
use stylo::values::generics::length::{AnchorSizeKeyword, GenericAnchorSizeFunction};
use stylo::values::generics::{NonNegative, Optional};
use stylo::values::specified::calc::{CalcNode as SpecifiedCalcNode, Leaf as SpecifiedLeaf};
use stylo::values::specified::length::NoCalcLength;
use stylo::values::{DashedIdent, specified};

use crate::geometry::{Edges, Size};
use crate::style::CoreStyle;
use crate::tree::LayoutTree;

/// The physical axis of the anchor an `anchor-size()` keyword measures, for a
/// function used in a property on `property_axis`.
///
/// css-anchor-position-1 §5.1: `width`/`height` are physical; `block`/`inline`
/// are the anchor's writing-mode axes and `self-block`/`self-inline` the query
/// box's; an omitted keyword is the axis of the property. With `writing-mode`
/// disabled every box is `horizontal-tb`, so both pairs of logical keywords
/// land on the same physical axes: block is vertical, inline horizontal.
#[must_use]
pub fn anchor_size_axis(keyword: AnchorSizeKeyword, property_axis: PhysicalAxis) -> PhysicalAxis {
    match keyword {
        AnchorSizeKeyword::None => property_axis,
        AnchorSizeKeyword::Width | AnchorSizeKeyword::Inline | AnchorSizeKeyword::SelfInline => {
            PhysicalAxis::Horizontal
        }
        AnchorSizeKeyword::Height | AnchorSizeKeyword::Block | AnchorSizeKeyword::SelfBlock => {
            PhysicalAxis::Vertical
        }
    }
}

/// The sizes of one box's target anchor elements, by name and physical axis.
pub(super) trait AnchorSizes {
    fn size(&mut self, name: &DashedIdent, axis: PhysicalAxis) -> Option<f32>;
}

/// The anchors of a box §5.1.1 resolves nothing for.
pub(super) struct Unresolvable;

impl AnchorSizes for Unresolvable {
    #[inline]
    fn size(&mut self, _name: &DashedIdent, _axis: PhysicalAxis) -> Option<f32> {
        None
    }
}

/// The anchors of an absolutely positioned box, as its host answers them in
/// the current pass.
struct HostAnchors<'a, T: LayoutTree> {
    tree: &'a T,
    state: &'a T::State,
    node: T::NodeId,
}

impl<T: LayoutTree> AnchorSizes for HostAnchors<'_, T> {
    fn size(&mut self, name: &DashedIdent, axis: PhysicalAxis) -> Option<f32> {
        let size = self.tree.anchor_size(self.state, self.node, name, axis)?;
        debug_assert!(
            size.is_finite() && size >= 0.0,
            "an anchor's border-box size must be finite and non-negative"
        );
        Some(size)
    }
}

/// What one `anchor-size()` resolves to.
enum Outcome<'a, V> {
    Resolved(f32),
    Fallback(&'a V),
    Invalid,
}

fn outcome<'a, V>(
    function: &'a GenericAnchorSizeFunction<V>,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> Outcome<'a, V> {
    let name = &function.target_element.value;
    // An omitted name selects the implicit anchor element (§5.1), which comes
    // from `position-anchor` — a property the fork does not parse — so there
    // is none.
    let resolved = if name.0.is_empty() {
        None
    } else {
        anchors.size(name, anchor_size_axis(function.size, property_axis))
    };
    match (resolved, &function.fallback) {
        (Some(size), _) => Outcome::Resolved(size),
        (None, Optional::Some(fallback)) => Outcome::Fallback(fallback),
        (None, Optional::None) => Outcome::Invalid,
    }
}

#[inline]
fn length(px: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(px))
}

/// A math function with every `anchor-size()` inside it replaced by the
/// resolved length or by its fallback; `None` when one of them has neither,
/// which makes the whole declaration invalid at computed-value time.
///
/// Stylo keeps the computed calc tree private, so the tree is reached through
/// its specified form: `from_computed_value` rebuilds it with absolute-length
/// and percentage leaves only, and those are exactly the leaves
/// `compute_without_context` turns back into a computed value.
fn substitute_calc(
    value: &LengthPercentage,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> Option<LengthPercentage> {
    let specified::LengthPercentage::Calc(calc) =
        specified::LengthPercentage::from_computed_value(value)
    else {
        // No math function left means no anchor function left either.
        return Some(value.clone());
    };
    let mut numeric = calc.0;
    substitute_node(&mut numeric.node, property_axis, anchors).ok()?;
    let substituted = specified::CalcLengthPercentage(numeric).compute_without_context();
    debug_assert!(
        substituted.is_some(),
        "a computed calc() rebuilt without anchor functions has only absolute leaves"
    );
    substituted
}

fn substitute_node(
    node: &mut SpecifiedCalcNode,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> Result<(), ()> {
    node.map_node(|node| match node {
        SpecifiedCalcNode::AnchorSize(function) => {
            match outcome(function, property_axis, anchors) {
                Outcome::Resolved(size) => Ok(Some(SpecifiedCalcNode::Leaf(
                    SpecifiedLeaf::Length(NoCalcLength::from_px(size)),
                ))),
                Outcome::Fallback(fallback) => {
                    let mut fallback = fallback.node.clone();
                    substitute_node(&mut fallback, property_axis, anchors)?;
                    Ok(Some(fallback))
                }
                Outcome::Invalid => Err(()),
            }
        }
        SpecifiedCalcNode::Anchor(_) => {
            unreachable!("the lynx fork rejects anchor() at parse time")
        }
        _ => Ok(None),
    })
}

/// A size with its anchor functions substituted.
pub(super) fn substitute_style_size(
    value: &StyleSize,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> StyleSize {
    match value {
        StyleSize::AnchorSizeFunction(function) => {
            match outcome(function, property_axis, anchors) {
                Outcome::Resolved(size) => StyleSize::LengthPercentage(NonNegative(length(size))),
                Outcome::Fallback(fallback) => {
                    substitute_style_size(fallback, property_axis, anchors)
                }
                Outcome::Invalid => StyleSize::Auto,
            }
        }
        StyleSize::AnchorContainingCalcFunction(calc) => {
            substitute_calc(&calc.0, property_axis, anchors).map_or(StyleSize::Auto, |value| {
                StyleSize::LengthPercentage(NonNegative(value))
            })
        }
        other => other.clone(),
    }
}

/// A max size with its anchor functions substituted.
pub(super) fn substitute_max_size(
    value: &MaxSize,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> MaxSize {
    match value {
        MaxSize::AnchorSizeFunction(function) => match outcome(function, property_axis, anchors) {
            Outcome::Resolved(size) => MaxSize::LengthPercentage(NonNegative(length(size))),
            Outcome::Fallback(fallback) => substitute_max_size(fallback, property_axis, anchors),
            Outcome::Invalid => MaxSize::None,
        },
        MaxSize::AnchorContainingCalcFunction(calc) => {
            substitute_calc(&calc.0, property_axis, anchors).map_or(MaxSize::None, |value| {
                MaxSize::LengthPercentage(NonNegative(value))
            })
        }
        other => other.clone(),
    }
}

/// A margin with its anchor functions substituted.
pub(super) fn substitute_margin(
    value: &Margin,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> Margin {
    match value {
        Margin::AnchorSizeFunction(function) => match outcome(function, property_axis, anchors) {
            Outcome::Resolved(size) => Margin::LengthPercentage(length(size)),
            Outcome::Fallback(fallback) => substitute_margin(fallback, property_axis, anchors),
            Outcome::Invalid => Margin::LengthPercentage(length(0.0)),
        },
        Margin::AnchorContainingCalcFunction(calc) => Margin::LengthPercentage(
            substitute_calc(calc, property_axis, anchors).unwrap_or_else(|| length(0.0)),
        ),
        other => other.clone(),
    }
}

/// An inset with its anchor functions substituted.
pub(super) fn substitute_inset(
    value: &Inset,
    property_axis: PhysicalAxis,
    anchors: &mut impl AnchorSizes,
) -> Inset {
    match value {
        Inset::AnchorSizeFunction(function) => match outcome(function, property_axis, anchors) {
            Outcome::Resolved(size) => Inset::LengthPercentage(length(size)),
            Outcome::Fallback(fallback) => substitute_inset(fallback, property_axis, anchors),
            Outcome::Invalid => Inset::Auto,
        },
        Inset::AnchorContainingCalcFunction(calc) => substitute_calc(calc, property_axis, anchors)
            .map_or(Inset::Auto, Inset::LengthPercentage),
        Inset::AnchorFunction(_) => unreachable!("the lynx fork rejects anchor() at parse time"),
        other => other.clone(),
    }
}

// The §5.1.1 unresolvable forms. Which axis a function would have measured is
// irrelevant when nothing resolves, so these pass an arbitrary one.

#[inline]
pub(super) fn unresolvable_style_size(value: &StyleSize) -> StyleSize {
    substitute_style_size(value, PhysicalAxis::Horizontal, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_max_size(value: &MaxSize) -> MaxSize {
    substitute_max_size(value, PhysicalAxis::Horizontal, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_margin(value: &Margin) -> Margin {
    substitute_margin(value, PhysicalAxis::Horizontal, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_inset(value: &Inset) -> Inset {
    substitute_inset(value, PhysicalAxis::Horizontal, &mut Unresolvable)
}

#[inline]
fn is_anchor_size(value: &StyleSize) -> bool {
    matches!(
        value,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_)
    )
}

#[inline]
fn is_anchor_max_size(value: &MaxSize) -> bool {
    matches!(
        value,
        MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_)
    )
}

#[inline]
fn is_anchor_margin(value: &Margin) -> bool {
    matches!(
        value,
        Margin::AnchorSizeFunction(_) | Margin::AnchorContainingCalcFunction(_)
    )
}

/// Whether an inset carries an anchor function, which may still resolve to
/// `auto` — the conservative answer for a caller deciding whether it needs a
/// static position.
#[inline]
pub(super) fn is_anchor_inset(value: &Inset) -> bool {
    matches!(
        value,
        Inset::AnchorFunction(_)
            | Inset::AnchorSizeFunction(_)
            | Inset::AnchorContainingCalcFunction(_)
    )
}

/// The geometry values an absolutely positioned box's layout reads, borrowed
/// either straight from its style or from its [`AnchoredGeometry`].
pub(super) struct GeometryValues<'a> {
    pub(super) size: Size<&'a StyleSize>,
    pub(super) min_size: Size<&'a StyleSize>,
    pub(super) max_size: Size<&'a MaxSize>,
    pub(super) margin: Edges<&'a Margin>,
    pub(super) inset: Edges<&'a Inset>,
}

impl<'a> GeometryValues<'a> {
    /// The values of a box: its substituted ones when it has any.
    #[inline]
    pub(super) fn of_box<S: CoreStyle>(
        style: &'a S,
        anchored: Option<&'a AnchoredGeometry>,
    ) -> Self {
        anchored.map_or_else(|| Self::of(style), AnchoredGeometry::values)
    }

    #[inline]
    pub(super) fn of<S: CoreStyle>(style: &'a S) -> Self {
        Self {
            size: style.size(),
            min_size: style.min_size(),
            max_size: style.max_size(),
            margin: style.margin(),
            inset: style.inset(),
        }
    }
}

/// An absolutely positioned box's sizes, min/max sizes, margins and insets
/// with every `anchor-size()` substituted by what §5.1.1 makes of it in the
/// current pass.
///
/// Owned, because the substituted values exist nowhere in the style; built
/// only for a box whose style references the function at all, so every other
/// box keeps borrowing its values straight from the style.
///
/// **Why this is enough, and why the box stays cacheable.** The box's own
/// algorithm still reads its style directly, and there the functions take the
/// unresolvable path. `absolute_layout` therefore hands the box every
/// anchor-dependent quantity its own run would read — on each
/// [`Self::sensitive`] axis, the used border-box size — as that axis's known
/// dimension, where the run takes it over its own style; insets and margins it
/// applies itself, outside the box's run. So the box's layout input carries
/// everything an anchor contributes, and its cache is keyed on it: a changed
/// anchor size is a changed input, a miss rather than a stale hit. What makes
/// the abs pass run again at all is the anchor's own change: a target is laid
/// out in the same containing block, so anything that moves its size clears
/// the containing block's cache on the way up, and the containing block's
/// committing run is the pass that lays the box out.
pub(super) struct AnchoredGeometry {
    size: Size<StyleSize>,
    min_size: Size<StyleSize>,
    max_size: Size<MaxSize>,
    margin: Edges<Margin>,
    inset: Edges<Inset>,
    /// Per axis: whether the box's own run would read an anchor-bearing value
    /// there — a size, a min or max size, or a margin on that axis (margins
    /// narrow the space an `auto` size fits into). An aspect ratio carries a
    /// size across, so with one either axis makes both sensitive.
    pub(super) sensitive: Size<bool>,
}

impl AnchoredGeometry {
    /// The substituted geometry of `node`, or `None` when its style references
    /// no anchor function — the common case, which allocates nothing.
    pub(super) fn resolve<T: LayoutTree>(
        tree: &T,
        state: &T::State,
        node: T::NodeId,
        style: &impl CoreStyle,
    ) -> Option<Self> {
        let size = style.size();
        let min_size = style.min_size();
        let max_size = style.max_size();
        let margin = style.margin();
        let inset = style.inset();
        let axis_references =
            |size: &StyleSize, min: &StyleSize, max: &MaxSize, start: &Margin, end: &Margin| {
                is_anchor_size(size)
                    || is_anchor_size(min)
                    || is_anchor_max_size(max)
                    || is_anchor_margin(start)
                    || is_anchor_margin(end)
            };
        let mut sensitive = Size::new(
            axis_references(
                size.width,
                min_size.width,
                max_size.width,
                margin.left,
                margin.right,
            ),
            axis_references(
                size.height,
                min_size.height,
                max_size.height,
                margin.top,
                margin.bottom,
            ),
        );
        let insets_reference = is_anchor_inset(inset.left)
            || is_anchor_inset(inset.right)
            || is_anchor_inset(inset.top)
            || is_anchor_inset(inset.bottom);
        if !sensitive.width && !sensitive.height && !insets_reference {
            return None;
        }
        if super::util::used_aspect_ratio(style.aspect_ratio()).is_some()
            && (sensitive.width || sensitive.height)
        {
            sensitive = Size::new(true, true);
        }

        let mut anchors = HostAnchors { tree, state, node };
        let (horizontal, vertical) = (PhysicalAxis::Horizontal, PhysicalAxis::Vertical);
        Some(Self {
            size: Size::new(
                substitute_style_size(size.width, horizontal, &mut anchors),
                substitute_style_size(size.height, vertical, &mut anchors),
            ),
            min_size: Size::new(
                substitute_style_size(min_size.width, horizontal, &mut anchors),
                substitute_style_size(min_size.height, vertical, &mut anchors),
            ),
            max_size: Size::new(
                substitute_max_size(max_size.width, horizontal, &mut anchors),
                substitute_max_size(max_size.height, vertical, &mut anchors),
            ),
            margin: Edges {
                left: substitute_margin(margin.left, horizontal, &mut anchors),
                right: substitute_margin(margin.right, horizontal, &mut anchors),
                top: substitute_margin(margin.top, vertical, &mut anchors),
                bottom: substitute_margin(margin.bottom, vertical, &mut anchors),
            },
            inset: Edges {
                left: substitute_inset(inset.left, horizontal, &mut anchors),
                right: substitute_inset(inset.right, horizontal, &mut anchors),
                top: substitute_inset(inset.top, vertical, &mut anchors),
                bottom: substitute_inset(inset.bottom, vertical, &mut anchors),
            },
            sensitive,
        })
    }

    #[inline]
    pub(super) fn values(&self) -> GeometryValues<'_> {
        GeometryValues {
            size: self.size.as_ref(),
            min_size: self.min_size.as_ref(),
            max_size: self.max_size.as_ref(),
            margin: self.margin.as_ref(),
            inset: self.inset.as_ref(),
        }
    }
}
