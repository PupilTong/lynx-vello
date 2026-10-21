//! css-anchor-position-1's anchor functions — `anchor()` (§3.2) and
//! `anchor-size()` (§5.1) — resolved the way §3.2.1 and §5.1.1 resolve them,
//! and the per-option geometry of an anchor-positioned box
//! ([`AnchoredGeometry`]): its substituted values, its `position-area`
//! region (§3.1, [`super::anchor_area`]), its self-alignment including
//! `anchor-center` (§4) and what §3.3 and §6.6 need to know about it.
//!
//! The computed style keeps the functions — `AnchorFunction` and
//! `AnchorSizeFunction` on their own, or `AnchorContainingCalcFunction` when
//! they sit inside a math function — in the sizes, min/max sizes, insets and
//! margins, and resolving them is layout's job. Both are *resolvable* only
//! when the box is absolutely positioned and a target anchor element exists
//! (for `anchor()`, also only in an inset on the axis of a physical side
//! keyword); otherwise the fallback applies, and without one the declaration
//! is invalid at computed-value time.
//!
//! Every resolver in [`super::util`] therefore handles the functions on its
//! own, and handles them as unresolvable: a value that reaches a resolver still
//! carrying a function is one no absolute pass substituted, so the box is not
//! one the functions resolve for. The absolutely positioned path substitutes
//! first ([`AnchoredGeometry::resolve`], called by the absolute pass), asking
//! the host for each target's border box through
//! [`LayoutTree::anchor_rect`].
//!
//! Invalid at computed-value time is approximated as the property's *initial*
//! value — `auto` for sizes and insets, `none` for max sizes, `0` for margins
//! — which is what it computes to for these non-inherited properties. The
//! approximation is that it is decided at layout time, per box, rather than
//! being visible in the computed value.
//!
//! Physical axes only: the fork disables `writing-mode`, so every box is
//! `horizontal-tb` — the block axis is vertical, the inline axis horizontal —
//! and only `direction` distinguishes inline-start from inline-end.

use smallvec::SmallVec;
use stylo::logical_geometry::{PhysicalAxis, PhysicalSide};
use stylo::values::computed::position::AnchorSide;
use stylo::values::computed::{
    Inset, Length, LengthPercentage, Margin, MaxSize, Size as StyleSize, ToComputedValue,
};
use stylo::values::generics::length::{AnchorSizeKeyword, GenericAnchorSizeFunction};
use stylo::values::generics::position::{
    AnchorSideKeyword, GenericAnchorFunction, GenericAnchorSide, TreeScoped,
};
use stylo::values::generics::{NonNegative, Optional};
use stylo::values::specified::align::AlignFlags;
use stylo::values::specified::calc::{CalcNode as SpecifiedCalcNode, Leaf as SpecifiedLeaf};
use stylo::values::specified::length::NoCalcLength;
use stylo::values::{DashedIdent, specified};

use super::anchor_area::{
    AreaKeywords, area_default_alignment, position_area_carried, position_area_region,
};
use super::{AbsoluteContainingBlock, AbsolutePlacement, AxisAlignment};
use crate::geometry::{Edges, Point, Rect, Size};
use crate::style::{CoreStyle, direction};
use crate::tree::{AnchorSpec, LayoutTree};

/// The physical axis of the anchor an `anchor-size()` keyword measures, for a
/// function used in a property on `property_axis`.
///
/// css-anchor-position-1 §5.1: `width`/`height` are physical; `block`/`inline`
/// are the anchor's writing-mode axes and `self-block`/`self-inline` the query
/// box's; an omitted keyword is the axis of the property. With `writing-mode`
/// disabled every box is `horizontal-tb`, so both pairs of logical keywords
/// land on the same physical axes: block is vertical, inline horizontal.
#[must_use]
pub(super) fn anchor_size_axis(
    keyword: AnchorSizeKeyword,
    property_axis: PhysicalAxis,
) -> PhysicalAxis {
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

/// The property a function is substituted in: its physical axis, and for an
/// inset its side — `anchor()` is allowed only there (§3.2).
#[derive(Clone, Copy)]
pub(super) enum Property {
    Axis(PhysicalAxis),
    Inset(PhysicalSide),
}

impl Property {
    #[inline]
    fn axis(self) -> PhysicalAxis {
        match self {
            Self::Axis(axis) => axis,
            Self::Inset(PhysicalSide::Left | PhysicalSide::Right) => PhysicalAxis::Horizontal,
            Self::Inset(PhysicalSide::Top | PhysicalSide::Bottom) => PhysicalAxis::Vertical,
        }
    }
}

/// An `<anchor-side>` with its percentage flattened to a fraction.
#[derive(Clone, Copy)]
pub(super) enum EdgeSide {
    Keyword(AnchorSideKeyword),
    Fraction(f32),
}

impl EdgeSide {
    fn of(side: AnchorSide) -> Self {
        match side {
            GenericAnchorSide::Keyword(keyword) => Self::Keyword(keyword),
            GenericAnchorSide::Percentage(percentage) => Self::Fraction(percentage.0),
        }
    }
}

/// What the anchor functions of one box resolve against.
pub(super) trait Anchors {
    /// §5.1.1: the size on `axis` of the target `name` selects.
    fn size(&mut self, name: &TreeScoped<DashedIdent>, axis: PhysicalAxis) -> Option<f32>;

    /// §3.2.1: the inset on `property` that aligns the inset-modified
    /// containing block's edge with the target's `side`.
    fn inset(
        &mut self,
        name: &TreeScoped<DashedIdent>,
        side: EdgeSide,
        property: PhysicalSide,
    ) -> Option<f32>;
}

/// The anchors of a box nothing resolves for.
pub(super) struct Unresolvable;

impl Anchors for Unresolvable {
    #[inline]
    fn size(&mut self, _name: &TreeScoped<DashedIdent>, _axis: PhysicalAxis) -> Option<f32> {
        None
    }

    #[inline]
    fn inset(
        &mut self,
        _name: &TreeScoped<DashedIdent>,
        _side: EdgeSide,
        _property: PhysicalSide,
    ) -> Option<f32> {
        None
    }
}

/// The spec an `<anchor-name>` selects: an omitted name is the default
/// anchor (§3.2, §5.1).
#[inline]
fn spec_of(name: &TreeScoped<DashedIdent>) -> AnchorSpec<'_> {
    if name.value.0.is_empty() {
        AnchorSpec::Default
    } else {
        AnchorSpec::Named(name)
    }
}

/// What one absolute layout of a box has asked its host, kept for the rest
/// of that layout.
///
/// The fallback loop lays one option out up to three times — §6.2's sort,
/// the trial, the commit — and several anchor functions of one option may
/// name the same anchor, so without this each question reaches the host's
/// §2.3 lookup once per use. The memo lives exactly as long as one call of
/// the absolute pass, never across a pass: the same pass can lay the box out
/// earlier, before its anchors are placed (a `linear` parent measures an
/// escaping box for its static position while it handles its own
/// out-of-flow children, `commit_non_in_flow_children`), and an answer kept
/// from there would outlive the anchors it missed.
#[derive(Debug, Default)]
pub(super) struct AnchorMemo {
    /// Each answered anchor rectangle, per option and name (`None` is the
    /// default anchor), in host coordinates.
    rects: SmallVec<[MemoRect; 4]>,
    /// The scrollable containing block, which no option changes, once asked.
    scrollable: MemoScrollable,
}

/// One answered rectangle: the option, the name (`None` for the default
/// anchor) and the host's answer.
type MemoRect = (usize, Option<TreeScoped<DashedIdent>>, Option<Rect<f32>>);

/// The scrollable containing block, before and after the host is asked.
#[derive(Debug, Default, Clone, Copy)]
enum MemoScrollable {
    #[default]
    Unasked,
    Answered(Option<Size<f32>>),
}

impl AnchorMemo {
    fn rect<T: LayoutTree>(
        &mut self,
        tree: &T,
        state: &T::State,
        node: T::NodeId,
        option: usize,
        spec: AnchorSpec<'_>,
    ) -> Option<Rect<f32>> {
        let name = match spec {
            AnchorSpec::Default => None,
            AnchorSpec::Named(name) => Some(name),
        };
        if let Some((.., known)) = self
            .rects
            .iter()
            .find(|(asked, asked_name, _)| *asked == option && asked_name.as_ref() == name)
        {
            return *known;
        }
        let rect = tree.anchor_rect(state, node, option, spec);
        debug_assert!(
            rect.is_none_or(|rect| rect.size.width.is_finite()
                && rect.size.height.is_finite()
                && rect.size.width >= 0.0
                && rect.size.height >= 0.0
                && rect.origin.x.is_finite()
                && rect.origin.y.is_finite()),
            "an anchor's border box must be finite with a non-negative size"
        );
        self.rects.push((option, name.cloned(), rect));
        rect
    }

    fn scrollable<T: LayoutTree>(
        &mut self,
        tree: &T,
        state: &T::State,
        node: T::NodeId,
    ) -> Option<Size<f32>> {
        if let MemoScrollable::Answered(answer) = self.scrollable {
            return answer;
        }
        let answer = tree.scrollable_containing_block(state, node);
        self.scrollable = MemoScrollable::Answered(answer);
        answer
    }
}

/// The anchors of an absolutely positioned box laid out with one position
/// option, as its host answers them in the current pass, plus what §3.3 and
/// §6.6 want to know about the references.
#[allow(
    clippy::struct_excessive_bools,
    reason = "two directions and two facts about the default anchor, not a state machine"
)]
struct HostAnchors<'a, T: LayoutTree> {
    tree: &'a T,
    state: &'a T::State,
    node: T::NodeId,
    option: usize,
    memo: &'a mut AnchorMemo,
    /// Host coordinates (the containing block generator's padding box) minus
    /// this, is the current containing block's coordinates: the grid area's
    /// and `position-area` region's offsets.
    origin: Point<f32>,
    /// The current containing block's size: the `position-area` region's,
    /// else the (scrollable) containing block's.
    containing_size: Size<f32>,
    containing_rtl: bool,
    self_rtl: bool,
    has_default_anchor: bool,
    references_default_anchor: bool,
    /// §3.3's third condition, per axis.
    compensates: Size<bool>,
}

impl<T: LayoutTree> HostAnchors<'_, T> {
    fn rect(&mut self, spec: AnchorSpec<'_>) -> Option<Rect<f32>> {
        if spec == AnchorSpec::Default {
            self.references_default_anchor = true;
        }
        let rect = self
            .memo
            .rect(self.tree, self.state, self.node, self.option, spec)?;
        Some(rect.translate(Point::new(-self.origin.x, -self.origin.y)))
    }
}

impl<T: LayoutTree> Anchors for HostAnchors<'_, T> {
    fn size(&mut self, name: &TreeScoped<DashedIdent>, axis: PhysicalAxis) -> Option<f32> {
        let rect = self.rect(spec_of(name))?;
        Some(match axis {
            PhysicalAxis::Horizontal => rect.size.width,
            PhysicalAxis::Vertical => rect.size.height,
        })
    }

    fn inset(
        &mut self,
        name: &TreeScoped<DashedIdent>,
        side: EdgeSide,
        property: PhysicalSide,
    ) -> Option<f32> {
        let horizontal = matches!(property, PhysicalSide::Left | PhysicalSide::Right);
        // §3.2.1: "If its <anchor-side> specifies a physical keyword, it's
        // specified in an inset property applicable to that axis."
        if let EdgeSide::Keyword(keyword) = side {
            let matching = match keyword {
                AnchorSideKeyword::Left | AnchorSideKeyword::Right => horizontal,
                AnchorSideKeyword::Top | AnchorSideKeyword::Bottom => !horizontal,
                _ => true,
            };
            if !matching {
                return None;
            }
        }
        let spec = spec_of(name);
        let rect = self.rect(spec)?;
        let (low, high) = if horizontal {
            (rect.origin.x, rect.origin.x + rect.size.width)
        } else {
            (rect.origin.y, rect.origin.y + rect.size.height)
        };
        // "start and end … resolving the keyword against the writing mode of
        // either the positioned box (for self-start and self-end) or the
        // positioned box's containing block (for start and end)". Only the
        // horizontal axis of an `rtl` box runs backwards.
        let start_end = |rtl: bool| {
            if horizontal && rtl {
                (high, low)
            } else {
                (low, high)
            }
        };
        let starts_low = matches!(property, PhysicalSide::Left | PhysicalSide::Top);
        let edge = match side {
            // "inside refers to the same side as the inset property …, while
            // outside refers to the opposite."
            EdgeSide::Keyword(AnchorSideKeyword::Inside) => {
                if starts_low {
                    low
                } else {
                    high
                }
            }
            EdgeSide::Keyword(AnchorSideKeyword::Outside) => {
                if starts_low {
                    high
                } else {
                    low
                }
            }
            EdgeSide::Keyword(AnchorSideKeyword::Left | AnchorSideKeyword::Top) => low,
            EdgeSide::Keyword(AnchorSideKeyword::Right | AnchorSideKeyword::Bottom) => high,
            EdgeSide::Keyword(AnchorSideKeyword::Start) => start_end(self.containing_rtl).0,
            EdgeSide::Keyword(AnchorSideKeyword::End) => start_end(self.containing_rtl).1,
            EdgeSide::Keyword(AnchorSideKeyword::SelfStart) => start_end(self.self_rtl).0,
            EdgeSide::Keyword(AnchorSideKeyword::SelfEnd) => start_end(self.self_rtl).1,
            // "Refers to a position a corresponding percentage between the
            // start and end sides, with 0% being equivalent to start and 100%
            // being equivalent to end. center is equivalent to 50%."
            EdgeSide::Keyword(AnchorSideKeyword::Center) => {
                let (start, end) = start_end(self.containing_rtl);
                start + 0.5 * (end - start)
            }
            EdgeSide::Fraction(fraction) => {
                let (start, end) = start_end(self.containing_rtl);
                start + fraction * (end - start)
            }
        };
        let axis = if horizontal {
            PhysicalAxis::Horizontal
        } else {
            PhysicalAxis::Vertical
        };
        if self.has_default_anchor {
            let compensates = match axis {
                PhysicalAxis::Horizontal => &mut self.compensates.width,
                PhysicalAxis::Vertical => &mut self.compensates.height,
            };
            if !*compensates {
                *compensates = match spec {
                    AnchorSpec::Default => true,
                    AnchorSpec::Named(name) => self.tree.anchor_scrolls_with_default(
                        self.state,
                        self.node,
                        self.option,
                        name,
                        axis,
                    ),
                };
            }
        }
        // "resolves … to the <length> that would align the edge of the
        // positioned boxes' inset-modified containing block corresponding to
        // the property the function appears in with the specified edge of the
        // target anchor element's anchor box."
        Some(match property {
            PhysicalSide::Left | PhysicalSide::Top => edge,
            PhysicalSide::Right => self.containing_size.width - edge,
            PhysicalSide::Bottom => self.containing_size.height - edge,
        })
    }
}

/// What one anchor function resolves to.
enum Outcome<'a, V> {
    Resolved(f32),
    Fallback(&'a V),
    Invalid,
}

#[inline]
fn settle<V>(resolved: Option<f32>, fallback: &Optional<V>) -> Outcome<'_, V> {
    match (resolved, fallback) {
        (Some(value), _) => Outcome::Resolved(value),
        (None, Optional::Some(fallback)) => Outcome::Fallback(fallback),
        (None, Optional::None) => Outcome::Invalid,
    }
}

fn size_outcome<'a, V>(
    function: &'a GenericAnchorSizeFunction<V>,
    property: Property,
    anchors: &mut impl Anchors,
) -> Outcome<'a, V> {
    let resolved = anchors.size(
        &function.target_element,
        anchor_size_axis(function.size, property.axis()),
    );
    settle(resolved, &function.fallback)
}

fn inset_outcome<'a, P, V>(
    function: &'a GenericAnchorFunction<P, V>,
    side: Option<EdgeSide>,
    property: Property,
    anchors: &mut impl Anchors,
) -> Outcome<'a, V> {
    // §3.2: "It is only allowed in the inset properties (and is otherwise
    // invalid)" — outside an inset, and with a side that did not flatten to
    // a keyword or a percentage, nothing resolves.
    let resolved = match (property, side) {
        (Property::Inset(property), Some(side)) => {
            anchors.inset(&function.target_element, side, property)
        }
        _ => None,
    };
    settle(resolved, &function.fallback)
}

#[inline]
fn length(px: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(px))
}

/// A math function with every anchor function inside it replaced by the
/// resolved length or by its fallback; `None` when one of them has neither,
/// which makes the whole declaration invalid at computed-value time.
///
/// Stylo keeps the computed calc tree private, so the tree is reached through
/// its specified form: `from_computed_value` rebuilds it with absolute-length
/// and percentage leaves only, and those are exactly the leaves
/// `compute_without_context` turns back into a computed value.
fn substitute_calc(
    value: &LengthPercentage,
    property: Property,
    anchors: &mut impl Anchors,
) -> Option<LengthPercentage> {
    let specified::LengthPercentage::Calc(calc) =
        specified::LengthPercentage::from_computed_value(value)
    else {
        // No math function left means no anchor function left either.
        return Some(value.clone());
    };
    let mut numeric = calc.0;
    substitute_node(&mut numeric.node, property, anchors).ok()?;
    let substituted = specified::CalcLengthPercentage(numeric).compute_without_context();
    debug_assert!(
        substituted.is_some(),
        "a computed calc() rebuilt without anchor functions has only absolute leaves"
    );
    substituted
}

/// The side of an `anchor()` inside a math function; its percentage is a
/// calc node the parser simplified to one leaf.
fn calc_side(side: &GenericAnchorSide<Box<SpecifiedCalcNode>>) -> Option<EdgeSide> {
    match side {
        GenericAnchorSide::Keyword(keyword) => Some(EdgeSide::Keyword(*keyword)),
        GenericAnchorSide::Percentage(node) => match node.resolve() {
            Ok(SpecifiedLeaf::Percentage(percentage)) => Some(EdgeSide::Fraction(percentage.get())),
            _ => None,
        },
    }
}

fn substitute_node(
    node: &mut SpecifiedCalcNode,
    property: Property,
    anchors: &mut impl Anchors,
) -> Result<(), ()> {
    let fallback_node = |fallback: &SpecifiedCalcNode, anchors: &mut _| {
        let mut fallback = fallback.clone();
        substitute_node(&mut fallback, property, anchors)?;
        Ok(Some(fallback))
    };
    node.map_node(|node| match node {
        SpecifiedCalcNode::AnchorSize(function) => {
            match size_outcome(function, property, anchors) {
                Outcome::Resolved(size) => Ok(Some(SpecifiedCalcNode::Leaf(
                    SpecifiedLeaf::Length(NoCalcLength::from_px(size)),
                ))),
                Outcome::Fallback(fallback) => fallback_node(&fallback.node, anchors),
                Outcome::Invalid => Err(()),
            }
        }
        SpecifiedCalcNode::Anchor(function) => {
            let side = calc_side(&function.side);
            match inset_outcome(function, side, property, anchors) {
                Outcome::Resolved(inset) => Ok(Some(SpecifiedCalcNode::Leaf(
                    SpecifiedLeaf::Length(NoCalcLength::from_px(inset)),
                ))),
                Outcome::Fallback(fallback) => fallback_node(&fallback.node, anchors),
                Outcome::Invalid => Err(()),
            }
        }
        _ => Ok(None),
    })
}

/// A size with its anchor functions substituted.
pub(super) fn substitute_style_size(
    value: &StyleSize,
    property: Property,
    anchors: &mut impl Anchors,
) -> StyleSize {
    match value {
        StyleSize::AnchorSizeFunction(function) => {
            match size_outcome(function, property, anchors) {
                Outcome::Resolved(size) => StyleSize::LengthPercentage(NonNegative(length(size))),
                Outcome::Fallback(fallback) => substitute_style_size(fallback, property, anchors),
                Outcome::Invalid => StyleSize::Auto,
            }
        }
        StyleSize::AnchorContainingCalcFunction(calc) => {
            substitute_calc(&calc.0, property, anchors).map_or(StyleSize::Auto, |value| {
                StyleSize::LengthPercentage(NonNegative(value))
            })
        }
        other => other.clone(),
    }
}

/// A max size with its anchor functions substituted.
pub(super) fn substitute_max_size(
    value: &MaxSize,
    property: Property,
    anchors: &mut impl Anchors,
) -> MaxSize {
    match value {
        MaxSize::AnchorSizeFunction(function) => match size_outcome(function, property, anchors) {
            Outcome::Resolved(size) => MaxSize::LengthPercentage(NonNegative(length(size))),
            Outcome::Fallback(fallback) => substitute_max_size(fallback, property, anchors),
            Outcome::Invalid => MaxSize::None,
        },
        MaxSize::AnchorContainingCalcFunction(calc) => substitute_calc(&calc.0, property, anchors)
            .map_or(MaxSize::None, |value| {
                MaxSize::LengthPercentage(NonNegative(value))
            }),
        other => other.clone(),
    }
}

/// A margin with its anchor functions substituted.
pub(super) fn substitute_margin(
    value: &Margin,
    property: Property,
    anchors: &mut impl Anchors,
) -> Margin {
    match value {
        Margin::AnchorSizeFunction(function) => match size_outcome(function, property, anchors) {
            Outcome::Resolved(size) => Margin::LengthPercentage(length(size)),
            Outcome::Fallback(fallback) => substitute_margin(fallback, property, anchors),
            Outcome::Invalid => Margin::LengthPercentage(length(0.0)),
        },
        Margin::AnchorContainingCalcFunction(calc) => Margin::LengthPercentage(
            substitute_calc(calc, property, anchors).unwrap_or_else(|| length(0.0)),
        ),
        other => other.clone(),
    }
}

/// An inset with its anchor functions substituted.
pub(super) fn substitute_inset(
    value: &Inset,
    property: Property,
    anchors: &mut impl Anchors,
) -> Inset {
    match value {
        Inset::AnchorSizeFunction(function) => match size_outcome(function, property, anchors) {
            Outcome::Resolved(size) => Inset::LengthPercentage(length(size)),
            Outcome::Fallback(fallback) => substitute_inset(fallback, property, anchors),
            Outcome::Invalid => Inset::Auto,
        },
        Inset::AnchorFunction(function) => {
            let side = Some(EdgeSide::of(function.side));
            match inset_outcome(function, side, property, anchors) {
                Outcome::Resolved(inset) => Inset::LengthPercentage(length(inset)),
                Outcome::Fallback(fallback) => substitute_inset(fallback, property, anchors),
                Outcome::Invalid => Inset::Auto,
            }
        }
        Inset::AnchorContainingCalcFunction(calc) => {
            substitute_calc(calc, property, anchors).map_or(Inset::Auto, Inset::LengthPercentage)
        }
        other => other.clone(),
    }
}

// The unresolvable forms. Which property a function would have resolved in is
// irrelevant when nothing resolves, so these pass an arbitrary one.

const ANY_PROPERTY: Property = Property::Axis(PhysicalAxis::Horizontal);

#[inline]
pub(super) fn unresolvable_style_size(value: &StyleSize) -> StyleSize {
    substitute_style_size(value, ANY_PROPERTY, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_max_size(value: &MaxSize) -> MaxSize {
    substitute_max_size(value, ANY_PROPERTY, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_margin(value: &Margin) -> Margin {
    substitute_margin(value, ANY_PROPERTY, &mut Unresolvable)
}

#[inline]
pub(super) fn unresolvable_inset(value: &Inset) -> Inset {
    substitute_inset(value, ANY_PROPERTY, &mut Unresolvable)
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

/// Whether an absolutely positioned box's style uses anything of
/// css-anchor-position-1 the absolute pass resolves: an anchor function in a
/// size, min/max size, margin or inset, a `position-area`,
/// `anchor-center` self-alignment, or a `position-anchor` that names an
/// element (which gives the box a default anchor, and with it the
/// scrollable containing block). One of the two style checks every
/// absolutely positioned box pays, beside
/// [`CoreStyle::has_position_try_fallbacks`]; a box that passes neither is
/// laid out exactly as before.
///
/// Public for hosts: exactly the boxes this answers `true` for (plus those
/// with position options) are reported through
/// [`LayoutTree::set_anchor_outcome`](crate::tree::LayoutTree::set_anchor_outcome),
/// so a host keeping per-box anchor state can tell a box that stopped being
/// anchor-positioned from one that simply was not laid out again.
#[inline]
pub fn uses_anchor_positioning(style: &impl CoreStyle) -> bool {
    let size = style.size();
    let min_size = style.min_size();
    let max_size = style.max_size();
    let margin = style.margin();
    let inset = style.inset();
    is_anchor_size(size.width)
        || is_anchor_size(size.height)
        || is_anchor_size(min_size.width)
        || is_anchor_size(min_size.height)
        || is_anchor_max_size(max_size.width)
        || is_anchor_max_size(max_size.height)
        || is_anchor_margin(margin.left)
        || is_anchor_margin(margin.right)
        || is_anchor_margin(margin.top)
        || is_anchor_margin(margin.bottom)
        || is_anchor_inset(inset.left)
        || is_anchor_inset(inset.right)
        || is_anchor_inset(inset.top)
        || is_anchor_inset(inset.bottom)
        || !style.position_area().is_none()
        || style.justify_self().0.value() == AlignFlags::ANCHOR_CENTER
        || style.align_self().0.value() == AlignFlags::ANCHOR_CENTER
        || style.names_position_anchor()
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

/// One absolutely positioned box's geometry under one position option, with
/// every anchor function substituted by what §3.2.1 and §5.1.1 make of it in
/// the current pass, `auto` insets and margins zeroed where `position-area`
/// and `anchor-center` zero them, and the containing block and
/// self-alignment the box is placed with.
///
/// Owned, because the substituted values exist nowhere in the style; built
/// only for a box that uses anchor positioning or is tried with a fallback
/// option, so every other box keeps borrowing its values straight from the
/// style.
///
/// **Why this is enough, and why the box stays cacheable.** The box's own
/// algorithm still reads its own (base) style directly, where the functions
/// take the unresolvable path and a fallback option's values do not exist.
/// The absolute pass therefore hands the box every quantity its own run
/// would read differently — on each [`AbsolutePlacement::sensitive`] axis,
/// the used border-box size — as that axis's known dimension, where the run
/// takes it over its own style; insets and margins it applies itself,
/// outside the box's run. So the box's layout input carries everything an
/// anchor or an option contributes, and its cache is keyed on it: a changed
/// anchor is a changed input, a miss rather than a stale hit. What makes the
/// absolute pass run again at all is the anchor's own change: a target of the
/// host's lookup is laid out in the same containing block, so anything that
/// moves it clears the containing block's cache on the way up, and the
/// containing block's committing run is the pass that lays the box out.
pub(super) struct AnchoredGeometry {
    size: Size<StyleSize>,
    min_size: Size<StyleSize>,
    max_size: Size<MaxSize>,
    margin: Edges<Margin>,
    inset: Edges<Inset>,
    pub(super) placement: AbsolutePlacement,
    /// [`crate::tree::AnchorOutcome::default_anchor_missing`].
    pub(super) default_anchor_missing: bool,
    /// §3.3, per axis.
    pub(super) compensates: Size<bool>,
    /// [`crate::tree::AnchorOutcome::carried_edges`].
    pub(super) carried: Edges<bool>,
}

impl AnchoredGeometry {
    /// The geometry of `node` under position option `option`, whose style is
    /// `style` (`base` is the box's own, and `base_uses` whether it
    /// [uses anchor positioning](uses_anchor_positioning)), or `None` for
    /// option `0` of a box that uses no anchor positioning at all — the
    /// common case, which allocates nothing.
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered pass over §3.1, §3.2, §4 and §5; splitting it scatters the order"
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "the absolute pass's inputs plus the option and its memo"
    )]
    pub(super) fn resolve<T: LayoutTree>(
        tree: &T,
        state: &T::State,
        node: T::NodeId,
        base: &impl CoreStyle,
        base_uses: bool,
        style: &impl CoreStyle,
        option: usize,
        containing_block: &AbsoluteContainingBlock,
        memo: &mut AnchorMemo,
    ) -> Option<Self> {
        if option == 0 && !base_uses {
            return None;
        }
        let self_rtl = base.direction() == direction::T::Rtl;
        let containing_rtl = containing_block.rtl;

        // §2.4: the default anchor. The host resolves `position-anchor`
        // under the option's style; its rectangle comes in the containing
        // block generator's padding-box coordinates.
        let default_rect = memo
            .rect(tree, state, node, option, AnchorSpec::Default)
            .map(|rect| {
                rect.translate(Point::new(
                    -containing_block.origin.x,
                    -containing_block.origin.y,
                ))
            });
        let has_default_anchor = default_rect.is_some();

        // css-position-4: with a default anchor, a containing block a scroll
        // container generates is its scrollable overflow area. §3.1.1's
        // "pre-modification containing block" is that one; css-position-3
        // §2.1.1's original containing block (the overflow limit's) is the
        // same unless grid placement narrowed it to a grid area.
        let scrollable = match default_rect {
            Some(_) if containing_block.is_padding_box => {
                let scrollable = memo.scrollable(tree, state, node);
                // The host's entry is clamped against the same padding box
                // by `record_scrollable_containing_block`, rewritten by every
                // committing run of the scroll container, and a cached
                // container keeps its padding box.
                debug_assert!(
                    scrollable
                        .is_none_or(|scrollable| scrollable.width >= containing_block.size.width
                            && scrollable.height >= containing_block.size.height),
                    "a scrollable containing block is never smaller than the padding box"
                );
                scrollable
            }
            _ => None,
        };
        let pre_modification = Rect::new(Point::ZERO, scrollable.unwrap_or(containing_block.size));
        let original = scrollable.map_or_else(
            || containing_block.original(),
            |size| Rect::new(Point::ZERO, size),
        );

        // §3.1: "If the box does not have a default anchor box, or is not an
        // absolutely positioned box, this value has no effect. Otherwise,
        // selects a region of the position-area grid, and makes that the
        // box's containing block."
        let position_area = style.position_area();
        let area_keywords = (!position_area.is_none())
            .then(|| AreaKeywords::physical(position_area, containing_rtl, self_rtl));
        let (area, area_keywords) = match (area_keywords, default_rect) {
            (Some(keywords), Some(anchor)) => (
                position_area_region(keywords, pre_modification, anchor),
                Some(keywords),
            ),
            _ => (pre_modification, None),
        };

        let mut anchors = HostAnchors {
            tree,
            state,
            node,
            option,
            memo,
            origin: Point::new(
                containing_block.origin.x + area.origin.x,
                containing_block.origin.y + area.origin.y,
            ),
            containing_size: area.size,
            containing_rtl,
            self_rtl,
            has_default_anchor,
            // "If the box references the default anchor box (e.g. using
            // position-area, …)".
            references_default_anchor: !position_area.is_none(),
            compensates: Size::new(false, false),
        };
        let (horizontal, vertical) = (
            Property::Axis(PhysicalAxis::Horizontal),
            Property::Axis(PhysicalAxis::Vertical),
        );
        let raw_size = style.size();
        let raw_min = style.min_size();
        let raw_max = style.max_size();
        let raw_margin = style.margin();
        let raw_inset = style.inset();
        let mut geometry = Self {
            size: Size::new(
                substitute_style_size(raw_size.width, horizontal, &mut anchors),
                substitute_style_size(raw_size.height, vertical, &mut anchors),
            ),
            min_size: Size::new(
                substitute_style_size(raw_min.width, horizontal, &mut anchors),
                substitute_style_size(raw_min.height, vertical, &mut anchors),
            ),
            max_size: Size::new(
                substitute_max_size(raw_max.width, horizontal, &mut anchors),
                substitute_max_size(raw_max.height, vertical, &mut anchors),
            ),
            margin: Edges {
                left: substitute_margin(raw_margin.left, horizontal, &mut anchors),
                right: substitute_margin(raw_margin.right, horizontal, &mut anchors),
                top: substitute_margin(raw_margin.top, vertical, &mut anchors),
                bottom: substitute_margin(raw_margin.bottom, vertical, &mut anchors),
            },
            inset: Edges {
                left: substitute_inset(
                    raw_inset.left,
                    Property::Inset(PhysicalSide::Left),
                    &mut anchors,
                ),
                right: substitute_inset(
                    raw_inset.right,
                    Property::Inset(PhysicalSide::Right),
                    &mut anchors,
                ),
                top: substitute_inset(
                    raw_inset.top,
                    Property::Inset(PhysicalSide::Top),
                    &mut anchors,
                ),
                bottom: substitute_inset(
                    raw_inset.bottom,
                    Property::Inset(PhysicalSide::Bottom),
                    &mut anchors,
                ),
            },
            placement: AbsolutePlacement {
                area,
                original,
                align: Size::new(
                    authored_alignment(style.justify_self().0, containing_block),
                    authored_alignment(style.align_self().0, containing_block),
                ),
                auto_inset: Edges {
                    left: false,
                    right: false,
                    top: false,
                    bottom: false,
                },
                sensitive: Size::new(false, false),
            },
            default_anchor_missing: false,
            compensates: Size::new(false, false),
            carried: Edges {
                left: false,
                right: false,
                top: false,
                bottom: false,
            },
        };
        let HostAnchors {
            mut references_default_anchor,
            mut compensates,
            ..
        } = anchors;
        // The computed `auto` insets, after an unresolvable function without
        // a fallback made one `auto`: they decide the weaker inset (css-
        // position-3 §3.5.2) and §4.1's single-`auto` rule even where
        // `position-area` makes their used value 0.
        let auto_inset = geometry
            .inset
            .as_ref()
            .map(|inset| matches!(inset, Inset::Auto));
        geometry.placement.auto_inset = auto_inset;
        // §6.5's fit test after a default scroll shift (Blink's
        // `CalculateNonOverflowingRangeInOneAxis`): an edge a non-`auto`
        // inset places keeps its relation to the box as laid out, and so does
        // a `position-area` line that is the anchor's own edge; only an
        // `auto` inset's containing-block edge stays where it is.
        let area_carried = match (area_keywords, default_rect) {
            (Some(keywords), Some(anchor)) => {
                position_area_carried(keywords, pre_modification, anchor)
            }
            _ => Edges {
                left: false,
                right: false,
                top: false,
                bottom: false,
            },
        };
        geometry.carried = Edges {
            left: !auto_inset.left || area_carried.left,
            right: !auto_inset.right || area_carried.right,
            top: !auto_inset.top || area_carried.top,
            bottom: !auto_inset.bottom || area_carried.bottom,
        };

        if let Some(keywords) = area_keywords {
            // §4.1: "When position-area is not none, the used value of normal
            // self-alignment changes depending on the <position-area> value".
            let align = &mut geometry.placement.align;
            if align.width.is_normal() {
                align.width.flags = area_default_alignment(
                    keywords,
                    PhysicalAxis::Horizontal,
                    (auto_inset.left, auto_inset.right),
                    containing_rtl,
                );
            }
            if align.height.is_normal() {
                align.height.flags = area_default_alignment(
                    keywords,
                    PhysicalAxis::Vertical,
                    (auto_inset.top, auto_inset.bottom),
                    containing_rtl,
                );
            }
            // §3.1: "The used value of any auto inset properties and auto
            // margin properties resolves to 0."
            geometry.zero_auto_edges(PhysicalAxis::Horizontal);
            geometry.zero_auto_edges(PhysicalAxis::Vertical);
            // §3.3: "abspos has a non-none value for position-area".
            compensates = Size::new(true, true);
        }

        // §4.2: "if the positioned box has a default anchor box, then it is
        // centered (insofar as possible) over the default anchor box in the
        // relevant axis. Additionally: The used value of any auto inset
        // properties and auto margin properties resolves to 0. If the box …
        // does not have a default anchor box, this value behaves as center and
        // has no additional effect on how inset properties resolve."
        for axis in [PhysicalAxis::Horizontal, PhysicalAxis::Vertical] {
            let alignment = match axis {
                PhysicalAxis::Horizontal => geometry.placement.align.width,
                PhysicalAxis::Vertical => geometry.placement.align.height,
            };
            if alignment.flags.value() != AlignFlags::ANCHOR_CENTER {
                continue;
            }
            references_default_anchor = true;
            let resolved = match default_rect {
                Some(anchor) => {
                    geometry.zero_auto_edges(axis);
                    match axis {
                        PhysicalAxis::Horizontal => compensates.width = true,
                        PhysicalAxis::Vertical => compensates.height = true,
                    }
                    let center = match axis {
                        PhysicalAxis::Horizontal => {
                            anchor.origin.x + anchor.size.width / 2.0 - area.origin.x
                        }
                        PhysicalAxis::Vertical => {
                            anchor.origin.y + anchor.size.height / 2.0 - area.origin.y
                        }
                    };
                    AxisAlignment {
                        flags: alignment.flags,
                        anchor_center: Some(center),
                    }
                }
                None => AxisAlignment {
                    flags: alignment.flags.with_value(AlignFlags::CENTER),
                    anchor_center: None,
                },
            };
            match axis {
                PhysicalAxis::Horizontal => geometry.placement.align.width = resolved,
                PhysicalAxis::Vertical => geometry.placement.align.height = resolved,
            }
        }
        // §3.3: "abspos has a default anchor box" is the first condition of
        // every compensation, and every writer above already requires one.
        geometry.compensates = compensates;
        // §6.6 `anchor-valid`.
        geometry.default_anchor_missing = references_default_anchor && !has_default_anchor;

        // The box's own run reads its base style: every axis on which the
        // values it lays out with differ from those is handed over as a known
        // dimension. An aspect ratio carries a size across the axes.
        let base_size = base.size();
        let base_min = base.min_size();
        let base_max = base.max_size();
        let base_margin = base.margin();
        let mut sensitive = Size::new(
            *base_size.width != geometry.size.width
                || *base_min.width != geometry.min_size.width
                || *base_max.width != geometry.max_size.width
                || *base_margin.left != geometry.margin.left
                || *base_margin.right != geometry.margin.right,
            *base_size.height != geometry.size.height
                || *base_min.height != geometry.min_size.height
                || *base_max.height != geometry.max_size.height
                || *base_margin.top != geometry.margin.top
                || *base_margin.bottom != geometry.margin.bottom,
        );
        if super::util::used_aspect_ratio(base.aspect_ratio()).is_some()
            && (sensitive.width || sensitive.height)
        {
            sensitive = Size::new(true, true);
        }
        geometry.placement.sensitive = sensitive;
        Some(geometry)
    }

    /// Zeroes the `auto` insets and margins on `axis`.
    fn zero_auto_edges(&mut self, axis: PhysicalAxis) {
        let zero_inset = |inset: &mut Inset| {
            if matches!(inset, Inset::Auto) {
                *inset = Inset::LengthPercentage(length(0.0));
            }
        };
        let zero_margin = |margin: &mut Margin| {
            if matches!(margin, Margin::Auto) {
                *margin = Margin::LengthPercentage(length(0.0));
            }
        };
        match axis {
            PhysicalAxis::Horizontal => {
                zero_inset(&mut self.inset.left);
                zero_inset(&mut self.inset.right);
                zero_margin(&mut self.margin.left);
                zero_margin(&mut self.margin.right);
            }
            PhysicalAxis::Vertical => {
                zero_inset(&mut self.inset.top);
                zero_inset(&mut self.inset.bottom);
                zero_margin(&mut self.margin.top);
                zero_margin(&mut self.margin.bottom);
            }
        }
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

    /// §6.2: the inset-modified containing block's size this option yields,
    /// "treating auto inset values as zero" — margins do not enter it.
    pub(super) fn sort_size(&self) -> Size<f32> {
        imcb_size_auto_zero(self.placement.area.size, self.inset.as_ref())
    }
}

/// The size of the inset-modified containing block `inset` cuts from a
/// containing block of size `area`, "treating auto inset values as zero"
/// (§6.2's sort key).
pub(super) fn imcb_size_auto_zero(area: Size<f32>, inset: Edges<&Inset>) -> Size<f32> {
    let used =
        |inset: &Inset, basis: f32| super::util::resolve_inset(inset, Some(basis)).unwrap_or(0.0);
    Size::new(
        (area.width - used(inset.left, area.width) - used(inset.right, area.width)).max(0.0),
        (area.height - used(inset.top, area.height) - used(inset.bottom, area.height)).max(0.0),
    )
}

/// The author's self-alignment of an absolutely positioned box: all of it
/// where the containing block honors self-alignment, only `anchor-center` in
/// a Lynx `linear` or `relative` container (see
/// [`AbsoluteContainingBlock::honors_self_alignment`]).
#[inline]
pub(super) fn authored_alignment(
    flags: AlignFlags,
    containing_block: &AbsoluteContainingBlock,
) -> AxisAlignment {
    if containing_block.honors_self_alignment || flags.value() == AlignFlags::ANCHOR_CENTER {
        AxisAlignment::of(flags)
    } else {
        AxisAlignment::NORMAL
    }
}

#[cfg(test)]
mod tests {
    use stylo::logical_geometry::PhysicalAxis::{Horizontal, Vertical};
    use stylo::values::generics::length::AnchorSizeKeyword;

    use super::anchor_size_axis;

    #[test]
    fn keywords_map_onto_physical_axes() {
        for property in [Horizontal, Vertical] {
            assert_eq!(
                anchor_size_axis(AnchorSizeKeyword::None, property),
                property
            );
            for keyword in [
                AnchorSizeKeyword::Width,
                AnchorSizeKeyword::Inline,
                AnchorSizeKeyword::SelfInline,
            ] {
                assert_eq!(anchor_size_axis(keyword, property), Horizontal);
            }
            for keyword in [
                AnchorSizeKeyword::Height,
                AnchorSizeKeyword::Block,
                AnchorSizeKeyword::SelfBlock,
            ] {
                assert_eq!(anchor_size_axis(keyword, property), Vertical);
            }
        }
    }
}
