//! Stylo-computed-value resolution helpers shared by layout entry points.

use stylo::computed_values::{box_sizing, direction};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::length_percentage::Unpacked as UnpackedLengthPercentage;
use stylo::values::computed::{
    AspectRatio, BorderSideWidth, Inset, Length, LengthPercentage, Margin, MaxSize,
    NonNegativeLengthPercentage, Overflow, Size as StyleSize,
};
use stylo::values::generics::position::PreferredRatio;
use stylo::values::specified::align::AlignFlags;

use crate::geometry::{Edges, Point, Size};
use crate::style::{Contain, CoreStyle};
use crate::tree::{AvailableSpace, LayoutInput, RequestedAxis, SizingMode};

/// Physical-axis projection shared by formatting algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
}

macro_rules! axis_projection {
    ($get:ident, $set:ident, $type:ident, $horizontal:ident, $vertical:ident) => {
        #[inline]
        pub(super) fn $get<T>(self, value: $type<T>) -> T {
            match self {
                Self::Horizontal => value.$horizontal,
                Self::Vertical => value.$vertical,
            }
        }

        #[inline]
        pub(super) fn $set<T>(self, value: &mut $type<T>, component: T) {
            match self {
                Self::Horizontal => value.$horizontal = component,
                Self::Vertical => value.$vertical = component,
            }
        }
    };
}

impl Axis {
    pub(super) const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    #[inline]
    pub(super) const fn other(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    axis_projection!(size, set_size, Size, width, height);
    axis_projection!(point, set_point, Point, x, y);
    axis_projection!(start, set_start, Edges, left, top);
    axis_projection!(end, set_end, Edges, right, bottom);

    #[inline]
    pub(super) fn sum(self, edges: Edges<f32>) -> f32 {
        self.start(edges) + self.end(edges)
    }

    #[inline]
    pub(super) fn pack<T>(self, along: T, across: T) -> Size<T> {
        match self {
            Self::Horizontal => Size::new(along, across),
            Self::Vertical => Size::new(across, along),
        }
    }

    #[inline]
    pub(super) const fn requested(self) -> RequestedAxis {
        match self {
            Self::Horizontal => RequestedAxis::Horizontal,
            Self::Vertical => RequestedAxis::Vertical,
        }
    }
}

#[inline]
fn normalize_alignment<const CONTENT: bool>(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignFlags> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        return None;
    }
    if !CONTENT && value == AlignFlags::LAST_BASELINE {
        return Some(AlignFlags::END);
    }
    let common = matches!(
        value,
        AlignFlags::START
            | AlignFlags::END
            | AlignFlags::FLEX_START
            | AlignFlags::FLEX_END
            | AlignFlags::CENTER
            | AlignFlags::STRETCH
    );
    let contextual = if CONTENT {
        matches!(
            value,
            AlignFlags::SPACE_BETWEEN | AlignFlags::SPACE_AROUND | AlignFlags::SPACE_EVENLY
        )
    } else {
        value == AlignFlags::BASELINE
    };
    if common || contextual {
        return Some(value);
    }
    match value {
        AlignFlags::LEFT if inline_axis && rtl => Some(AlignFlags::END),
        AlignFlags::RIGHT if inline_axis && !rtl => Some(AlignFlags::END),
        AlignFlags::LEFT | AlignFlags::RIGHT if inline_axis => Some(AlignFlags::START),
        _ => Some(AlignFlags::START),
    }
}

#[inline]
pub(super) fn normalize_item_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignFlags> {
    normalize_alignment::<false>(flags, inline_axis, rtl)
}

#[inline]
pub(super) fn normalize_content_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignFlags> {
    normalize_alignment::<true>(flags, inline_axis, rtl)
}

/// Node handle and order-modified paint index retained by resolved item
/// scratch.
#[derive(Debug, Clone, Copy)]
pub(super) struct ItemKey<N> {
    pub(super) node: N,
    pub(super) layout_order: u32,
}

/// Source-order and paint-order metadata collected for every generated child.
///
/// The field order intentionally packs this to 24 bytes on 64-bit targets
/// with a one-word handle (32 with a two-word handle).
#[derive(Debug, Clone, Copy)]
pub(super) struct OrderedItem<N> {
    pub(super) node: N,
    pub(super) document_index: usize,
    pub(super) css_order: i32,
    pub(super) layout_order: u32,
}

impl<N: Copy> OrderedItem<N> {
    #[inline]
    pub(super) const fn key(self) -> ItemKey<N> {
        ItemKey {
            node: self.node,
            layout_order: self.layout_order,
        }
    }
}

/// Access to ordering metadata embedded in algorithm-specific item scratch.
pub(super) trait PendingLayoutItem<N> {
    fn ordered(&self) -> &OrderedItem<N>;
    fn ordered_mut(&mut self) -> &mut OrderedItem<N>;
}

impl<N> PendingLayoutItem<N> for OrderedItem<N> {
    #[inline]
    fn ordered(&self) -> &OrderedItem<N> {
        self
    }

    #[inline]
    fn ordered_mut(&mut self) -> &mut OrderedItem<N> {
        self
    }
}

pub(super) fn sort_and_assign_layout_order<N, InFlow, OutOfFlow>(
    in_flow: &mut [InFlow],
    out_of_flow: &mut [OutOfFlow],
) where
    InFlow: PendingLayoutItem<N>,
    OutOfFlow: PendingLayoutItem<N>,
{
    let has_modified_order = in_flow.iter().any(|item| item.ordered().css_order != 0);
    if has_modified_order {
        in_flow.sort_unstable_by_key(|item| {
            let ordered = item.ordered();
            (ordered.css_order, ordered.document_index)
        });

        let mut out_of_flow_before = 0;
        for (in_flow_order, item) in in_flow.iter_mut().enumerate() {
            let ordered = item.ordered();
            let count = match ordered.css_order.cmp(&0) {
                core::cmp::Ordering::Less => 0,
                core::cmp::Ordering::Greater => out_of_flow.len(),
                core::cmp::Ordering::Equal => {
                    while out_of_flow_before < out_of_flow.len()
                        && out_of_flow[out_of_flow_before].ordered().document_index
                            < ordered.document_index
                    {
                        out_of_flow_before += 1;
                    }
                    out_of_flow_before
                }
            };
            item.ordered_mut().layout_order =
                u32::try_from(in_flow_order.saturating_add(count)).unwrap_or(u32::MAX);
        }

        let mut in_flow_before = 0;
        for (out_of_flow_order, item) in out_of_flow.iter_mut().enumerate() {
            let document_index = item.ordered().document_index;
            while in_flow_before < in_flow.len() {
                let ordered = in_flow[in_flow_before].ordered();
                if (ordered.css_order, ordered.document_index) >= (0, document_index) {
                    break;
                }
                in_flow_before += 1;
            }
            item.ordered_mut().layout_order =
                u32::try_from(in_flow_before.saturating_add(out_of_flow_order)).unwrap_or(u32::MAX);
        }
        return;
    }

    let (mut in_flow_index, mut out_of_flow_index, mut layout_order) = (0, 0, 0_u32);
    while in_flow_index < in_flow.len() || out_of_flow_index < out_of_flow.len() {
        let take_in_flow = out_of_flow_index == out_of_flow.len()
            || (in_flow_index < in_flow.len()
                && in_flow[in_flow_index].ordered().document_index
                    < out_of_flow[out_of_flow_index].ordered().document_index);
        if take_in_flow {
            in_flow[in_flow_index].ordered_mut().layout_order = layout_order;
            in_flow_index += 1;
        } else {
            out_of_flow[out_of_flow_index].ordered_mut().layout_order = layout_order;
            out_of_flow_index += 1;
        }
        layout_order = layout_order.saturating_add(1);
    }
}

/// Compact auto-edge mask retained with resolved item geometry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub(super) struct EdgeMask(u8);

impl EdgeMask {
    #[inline]
    pub(super) fn from_margins(value: Edges<&Margin>) -> Self {
        Self(
            u8::from(value.left.is_auto())
                | u8::from(value.right.is_auto()) << 1
                | u8::from(value.top.is_auto()) << 2
                | u8::from(value.bottom.is_auto()) << 3,
        )
    }

    #[cfg(test)]
    #[inline]
    pub(super) const fn from_edges(value: Edges<bool>) -> Self {
        Self(
            value.left as u8
                | (value.right as u8) << 1
                | (value.top as u8) << 2
                | (value.bottom as u8) << 3,
        )
    }

    #[inline]
    pub(super) const fn start(self, axis: Axis) -> bool {
        self.0 & (1 << Self::axis_shift(axis)) != 0
    }

    #[inline]
    pub(super) const fn end(self, axis: Axis) -> bool {
        self.0 & (2 << Self::axis_shift(axis)) != 0
    }

    #[inline]
    pub(super) const fn flow_start(self, axis: Axis, reverse: bool) -> bool {
        self.edge(axis, reverse)
    }

    #[inline]
    pub(super) const fn flow_end(self, axis: Axis, reverse: bool) -> bool {
        self.edge(axis, !reverse)
    }

    const fn axis_shift(axis: Axis) -> u8 {
        match axis {
            Axis::Horizontal => 0,
            Axis::Vertical => 2,
        }
    }

    const fn edge(self, axis: Axis, end: bool) -> bool {
        self.0 & ((1 + end as u8) << Self::axis_shift(axis)) != 0
    }
}

/// Intrinsic keyword occupying one two-bit slot in [`IntrinsicTags`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum IntrinsicTag {
    None,
    MinContent,
    MaxContent,
    FitContent,
}

impl IntrinsicTag {
    #[inline]
    fn from_style_size(value: &StyleSize) -> Self {
        if matches!(value, StyleSize::MinContent) {
            Self::MinContent
        } else if matches!(value, StyleSize::MaxContent) {
            Self::MaxContent
        } else if matches!(value, StyleSize::FitContentFunction(_)) {
            Self::FitContent
        } else {
            debug_assert!(!matches!(
                value,
                StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_)
            ));
            Self::None
        }
    }

    #[inline]
    fn from_max_size(value: &MaxSize) -> Self {
        if matches!(value, MaxSize::MinContent) {
            Self::MinContent
        } else if matches!(value, MaxSize::MaxContent) {
            Self::MaxContent
        } else if matches!(value, MaxSize::FitContentFunction(_)) {
            Self::FitContent
        } else {
            debug_assert!(!matches!(
                value,
                MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_)
            ));
            Self::None
        }
    }

    #[inline]
    pub(super) const fn is_intrinsic(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Six intrinsic keyword classifications packed into twelve bits.
///
/// Fit-content payloads remain in host-owned computed style and are
/// re-borrowed only by the algorithm pass that consumes them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub(super) struct IntrinsicTags(u16);

impl IntrinsicTags {
    #[inline]
    pub(super) fn new(
        size: Size<&StyleSize>,
        min_size: Size<&StyleSize>,
        max_size: Size<&MaxSize>,
    ) -> Self {
        Self(
            Self::pair(
                IntrinsicTag::from_style_size(size.width),
                IntrinsicTag::from_style_size(size.height),
            ) | Self::pair(
                IntrinsicTag::from_style_size(min_size.width),
                IntrinsicTag::from_style_size(min_size.height),
            ) << 4
                | Self::pair(
                    IntrinsicTag::from_max_size(max_size.width),
                    IntrinsicTag::from_max_size(max_size.height),
                ) << 8,
        )
    }

    #[inline]
    pub(super) const fn preferred(self, axis: Axis) -> IntrinsicTag {
        self.at(axis, 0)
    }

    #[inline]
    pub(super) const fn minimum(self, axis: Axis) -> IntrinsicTag {
        self.at(axis, 4)
    }

    #[inline]
    pub(super) const fn maximum(self, axis: Axis) -> IntrinsicTag {
        self.at(axis, 8)
    }

    const fn at(self, axis: Axis, offset: u32) -> IntrinsicTag {
        match (self.0 >> (Self::axis_shift(axis) + offset)) & 0b11 {
            0 => IntrinsicTag::None,
            1 => IntrinsicTag::MinContent,
            2 => IntrinsicTag::MaxContent,
            3 => IntrinsicTag::FitContent,
            _ => unreachable!(),
        }
    }

    #[inline]
    pub(super) const fn has_intrinsic(self) -> bool {
        self.0 != 0
    }

    #[inline]
    pub(super) const fn needs_min_content(self, axis: Axis) -> bool {
        self.0 & (0x111 << Self::axis_shift(axis)) != 0
    }

    #[inline]
    pub(super) const fn needs_max_content(self, axis: Axis) -> bool {
        self.0 & (0x222 << Self::axis_shift(axis)) != 0
    }

    const fn pair(width: IntrinsicTag, height: IntrinsicTag) -> u16 {
        width as u16 | (height as u16) << 2
    }

    const fn axis_shift(axis: Axis) -> u32 {
        match axis {
            Axis::Horizontal => 0,
            Axis::Vertical => 2,
        }
    }
}

/// Owned used geometry embedded directly in every algorithm's item record.
///
/// No raw Stylo value or fit-content payload crosses the resolver boundary.
#[derive(Debug, Clone, Copy)]
pub(super) struct ItemGeometry {
    pub(super) preferred_size: Size<Option<f32>>,
    pub(super) min_size: Size<Option<f32>>,
    pub(super) max_size: Size<Option<f32>>,
    pub(super) margin: Edges<f32>,
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) intrinsic: IntrinsicTags,
    pub(super) preferred_definite: Size<bool>,
    pub(super) size_is_auto: Size<bool>,
    pub(super) overflow: Point<Overflow>,
    pub(super) box_sizing: box_sizing::T,
    pub(super) margin_auto: EdgeMask,
}

impl ItemGeometry {
    #[inline]
    pub(super) fn box_floor(&self) -> Size<f32> {
        box_inset_size(self.padding, self.border)
    }
}

macro_rules! impl_item_geometry {
    ($item:ident) => {
        impl<N> core::ops::Deref for $item<N> {
            type Target = ItemGeometry;

            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.geometry
            }
        }

        impl<N> core::ops::DerefMut for $item<N> {
            #[inline]
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.geometry
            }
        }
    };
}
pub(super) use impl_item_geometry;

/// Algorithm-neutral resolved container box and sizing constraints.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedContainerBox {
    pub(super) preferred_definite: Size<bool>,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) box_sizing: box_sizing::T,
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) box_inset: Size<f32>,
    pub(super) min: Size<Option<f32>>,
    pub(super) max: Size<Option<f32>>,
    pub(super) outer: Size<Option<f32>>,
    pub(super) inner: Size<Option<f32>>,
    pub(super) available_inner: Size<AvailableSpace>,
}

#[inline]
fn checked(value: f32) -> f32 {
    debug_assert!(value.is_finite(), "layout values must be finite");
    value
}

#[inline]
pub(super) fn resolve_length_percentage(
    value: &LengthPercentage,
    basis: Option<f32>,
) -> Option<f32> {
    let length = match basis {
        Some(basis) => value.resolve(Length::new(basis)),
        None => match value.unpack() {
            UnpackedLengthPercentage::Length(length) => length,
            UnpackedLengthPercentage::Percentage(_) | UnpackedLengthPercentage::Calc(_) => {
                return None;
            }
        },
    };
    Some(checked(length.px()))
}

#[inline]
pub(super) fn resolve_margin(value: &Margin, basis: Option<f32>) -> Option<f32> {
    match value {
        Margin::LengthPercentage(lp) => resolve_length_percentage(lp, basis),
        Margin::Auto => None,
        Margin::AnchorSizeFunction(_) | Margin::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor margins are pref-dead under the lynx feature")
        }
    }
}

#[inline]
pub(super) fn resolve_inset(value: &Inset, basis: Option<f32>) -> Option<f32> {
    match value {
        Inset::LengthPercentage(lp) => resolve_length_percentage(lp, basis),
        Inset::Auto => None,
        Inset::AnchorFunction(_)
        | Inset::AnchorSizeFunction(_)
        | Inset::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor insets are pref-dead under the lynx feature")
        }
    }
}

#[inline]
pub(super) fn resolve_style_size(value: &StyleSize, basis: Option<f32>) -> Option<f32> {
    match value {
        StyleSize::LengthPercentage(lp) => resolve_length_percentage(&lp.0, basis),
        StyleSize::Auto
        | StyleSize::MinContent
        | StyleSize::MaxContent
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable
        | StyleSize::FitContentFunction(_) => None,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

#[inline]
pub(super) fn resolve_max_size(value: &MaxSize, basis: Option<f32>) -> Option<f32> {
    match value {
        MaxSize::LengthPercentage(lp) => resolve_length_percentage(&lp.0, basis),
        MaxSize::None
        | MaxSize::MinContent
        | MaxSize::MaxContent
        | MaxSize::FitContent
        | MaxSize::Stretch
        | MaxSize::WebkitFillAvailable
        | MaxSize::FitContentFunction(_) => None,
        MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

#[inline]
pub(super) fn resolve_size(value: Size<&StyleSize>, basis: Size<Option<f32>>) -> Size<Option<f32>> {
    value.zip_map(basis, resolve_style_size)
}

#[inline]
pub(super) fn resolve_max_sizes(
    value: Size<&MaxSize>,
    basis: Size<Option<f32>>,
) -> Size<Option<f32>> {
    value.zip_map(basis, resolve_max_size)
}

#[inline]
pub(super) fn resolve_padding(
    value: Edges<&NonNegativeLengthPercentage>,
    inline_basis: Option<f32>,
) -> Edges<f32> {
    value.map(|edge| resolve_padding_edge(edge, inline_basis))
}

#[inline]
fn resolve_padding_edge(value: &NonNegativeLengthPercentage, inline_basis: Option<f32>) -> f32 {
    resolve_length_percentage(&value.0, inline_basis)
        .unwrap_or(0.0)
        .max(0.0)
}

#[inline]
pub(super) fn resolve_border(value: &Edges<BorderSideWidth>) -> Edges<f32> {
    value
        .as_ref()
        .map(|edge| checked(edge.0.to_f32_px()).max(0.0))
}

#[inline]
pub(super) fn resolve_margins(
    value: Edges<&Margin>,
    inline_basis: Option<f32>,
) -> Edges<Option<f32>> {
    value.map(|edge| resolve_margin(edge, inline_basis))
}

#[inline]
pub(super) fn auto_edges_to_zero(value: Edges<Option<f32>>) -> Edges<f32> {
    value.map(|side| side.unwrap_or(0.0))
}

#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a per-item call after the shared box resolver is inlined"
)]
pub(super) fn resolve_insets(value: Edges<&Inset>, basis: Size<Option<f32>>) -> Edges<Option<f32>> {
    Edges {
        left: resolve_inset(value.left, basis.width),
        right: resolve_inset(value.right, basis.width),
        top: resolve_inset(value.top, basis.height),
        bottom: resolve_inset(value.bottom, basis.height),
    }
}

#[inline]
pub(super) fn add_optional_sizes(value: Size<Option<f32>>, amount: Size<f32>) -> Size<Option<f32>> {
    Size::new(
        value.width.map(|value| value + amount.width),
        value.height.map(|value| value + amount.height),
    )
}

#[inline]
pub(super) fn apply_box_sizing(
    value: Size<Option<f32>>,
    box_sizing: box_sizing::T,
    padding_border_size: Size<f32>,
) -> Size<Option<f32>> {
    if box_sizing == box_sizing::T::ContentBox {
        add_optional_sizes(value, padding_border_size)
    } else {
        value
    }
}

#[inline]
pub(super) fn apply_aspect_ratio(
    mut value: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<Option<f32>> {
    let Some(ratio) = aspect_ratio else {
        return value;
    };
    debug_assert!(
        ratio.is_finite() && ratio > 0.0,
        "aspect-ratio must be positive and finite"
    );
    if !ratio.is_finite() || ratio <= 0.0 {
        return value;
    }

    match (value.width, value.height) {
        (Some(width), None) => value.height = Some(width / ratio),
        (None, Some(height)) => value.width = Some(height * ratio),
        _ => {}
    }
    value
}

#[inline]
pub(super) fn used_aspect_ratio(value: AspectRatio) -> Option<f32> {
    match value.ratio {
        PreferredRatio::None => None,
        PreferredRatio::Ratio(ratio) => (!ratio.is_degenerate()).then(|| ratio.0.0 / ratio.1.0),
    }
}

#[inline]
pub(super) fn style_size_behaves_auto(value: &StyleSize) -> bool {
    match value {
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
        _ => matches!(
            value,
            StyleSize::Auto
                | StyleSize::FitContent
                | StyleSize::Stretch
                | StyleSize::WebkitFillAvailable
        ),
    }
}

#[inline]
fn fit_content_size(
    limit: &LengthPercentage,
    min_content: Option<f32>,
    max_content: Option<f32>,
    basis: Option<f32>,
    inset: f32,
    box_sizing: box_sizing::T,
) -> f32 {
    let min_content = min_content.unwrap_or(0.0);
    let max_content = max_content.unwrap_or(min_content);
    let mut limit = resolve_length_percentage(limit, basis).unwrap_or(max_content);
    if box_sizing == box_sizing::T::ContentBox {
        limit += inset;
    }
    max_content.min(limit.max(min_content))
}

pub(super) trait IntrinsicValue {
    fn fit_content_limit(&self) -> &LengthPercentage;
}

impl IntrinsicValue for StyleSize {
    fn fit_content_limit(&self) -> &LengthPercentage {
        let Self::FitContentFunction(limit) = self else {
            unreachable!("only the fit-content tag requests its payload")
        };
        &limit.0
    }
}

impl IntrinsicValue for MaxSize {
    fn fit_content_limit(&self) -> &LengthPercentage {
        let Self::FitContentFunction(limit) = self else {
            unreachable!("only the fit-content tag requests its payload")
        };
        &limit.0
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_intrinsic(
    tag: IntrinsicTag,
    value: &impl IntrinsicValue,
    quantitative: Option<f32>,
    min_content: Option<f32>,
    max_content: Option<f32>,
    basis: Option<f32>,
    inset: f32,
    box_sizing: box_sizing::T,
) -> Option<f32> {
    match tag {
        IntrinsicTag::None => quantitative,
        IntrinsicTag::MinContent => min_content,
        IntrinsicTag::MaxContent => max_content,
        IntrinsicTag::FitContent => Some(fit_content_size(
            value.fit_content_limit(),
            min_content,
            max_content,
            basis,
            inset,
            box_sizing,
        )),
    }
}

#[inline]
fn style_size_is_definite(value: &StyleSize, parent_basis: Option<f32>) -> bool {
    match value {
        StyleSize::LengthPercentage(lp) => !lp.0.has_percentage() || parent_basis.is_some(),
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
        _ => false,
    }
}

#[inline]
pub(super) fn preferred_size_definiteness(
    size: Size<&StyleSize>,
    parent_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<bool> {
    let mut definite = size.zip_map(parent_size, style_size_is_definite);
    if aspect_ratio.is_some() {
        if definite.width {
            definite.height = true;
        } else if definite.height {
            definite.width = true;
        }
    }
    definite
}

#[inline]
pub(super) fn clamp(value: f32, min: Option<f32>, max: Option<f32>) -> f32 {
    value
        .min(max.unwrap_or(f32::INFINITY))
        .max(min.unwrap_or(0.0))
}

#[inline]
pub(super) fn relative_offset(inset: Edges<Option<f32>>, direction: direction::T) -> Point<f32> {
    let x = match (inset.left, inset.right) {
        (Some(_), Some(right)) if direction == direction::T::Rtl => -right,
        (Some(left), _) => left,
        (None, Some(right)) => -right,
        (None, None) => 0.0,
    };
    let y = inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0));
    Point::new(x, y)
}

#[inline]
pub(super) fn box_inset_size(padding: Edges<f32>, border: Edges<f32>) -> Size<f32> {
    Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    )
}

#[inline]
pub(super) fn resolve_quantitative_sizes(
    value: Size<&StyleSize>,
    basis: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
    box_inset: Size<f32>,
) -> Size<Option<f32>> {
    apply_box_sizing(
        apply_aspect_ratio(resolve_size(value, basis), aspect_ratio),
        box_sizing,
        box_inset,
    )
}

#[inline]
pub(super) fn resolve_quantitative_max_sizes(
    value: Size<&MaxSize>,
    basis: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
    box_inset: Size<f32>,
) -> Size<Option<f32>> {
    apply_box_sizing(
        apply_aspect_ratio(resolve_max_sizes(value, basis), aspect_ratio),
        box_sizing,
        box_inset,
    )
}

#[inline]
pub(super) fn clamp_axis(value: f32, min: Option<f32>, max: Option<f32>, floor: f32) -> f32 {
    clamp(value, min, max).max(floor)
}

#[inline]
pub(super) fn subtract_available_space(
    available_space: AvailableSpace,
    amount: f32,
) -> AvailableSpace {
    match available_space {
        AvailableSpace::Definite(value) => AvailableSpace::Definite((value - amount).max(0.0)),
        intrinsic => intrinsic,
    }
}

#[inline]
pub(super) fn resolve_gap_axis(
    value: &NonNegativeLengthPercentageOrNormal,
    basis: Option<f32>,
) -> f32 {
    match value {
        NonNegativeLengthPercentageOrNormal::Normal => 0.0,
        NonNegativeLengthPercentageOrNormal::LengthPercentage(lp) => {
            resolve_length_percentage(&lp.0, basis)
                .unwrap_or(0.0)
                .max(0.0)
        }
    }
}

#[inline]
pub(super) fn resolve_gap(
    value: Size<&NonNegativeLengthPercentageOrNormal>,
    basis: Size<Option<f32>>,
) -> Size<f32> {
    value.zip_map(basis, resolve_gap_axis)
}

#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a large resolver result and copy chain in release LLVM IR"
)]
pub(super) fn resolve_item_geometry(
    style: &impl CoreStyle,
    percentage_basis: Size<Option<f32>>,
) -> ItemGeometry {
    resolve_item_geometry_with_bases(style, percentage_basis, percentage_basis.width)
}

#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a large resolver result and copy chain in release LLVM IR"
)]
pub(super) fn resolve_item_geometry_with_bases(
    style: &impl CoreStyle,
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
) -> ItemGeometry {
    let raw_size = style.size();
    let raw_min_size = style.min_size();
    let raw_max_size = style.max_size();
    let aspect_ratio = used_aspect_ratio(style.aspect_ratio());
    let box_sizing = style.box_sizing();
    let overflow = style.overflow();
    let padding_value = style.padding();
    let border_value = style.border();
    let padding = resolve_padding(padding_value, edge_inline_basis);
    let border = resolve_border(&border_value);
    let box_inset = box_inset_size(padding, border);
    let preferred_size = resolve_quantitative_sizes(
        raw_size,
        size_percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
    );
    let min_size = resolve_quantitative_sizes(
        raw_min_size,
        size_percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
    );
    let max_size = resolve_quantitative_max_sizes(
        raw_max_size,
        size_percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
    );
    let margin_value = style.margin();
    let optional_margin = resolve_margins(margin_value, edge_inline_basis);

    ItemGeometry {
        preferred_size,
        min_size,
        max_size,
        margin: auto_edges_to_zero(optional_margin),
        padding,
        border,
        aspect_ratio,
        intrinsic: IntrinsicTags::new(raw_size, raw_min_size, raw_max_size),
        preferred_definite: preferred_size_definiteness(
            raw_size,
            size_percentage_basis,
            aspect_ratio,
        ),
        size_is_auto: Size::new(
            style_size_behaves_auto(raw_size.width),
            style_size_behaves_auto(raw_size.height),
        ),
        overflow,
        box_sizing,
        margin_auto: EdgeMask::from_margins(margin_value),
    }
}

#[inline]
pub(super) fn resolve_container_box(
    style: &impl CoreStyle,
    input: LayoutInput,
) -> ResolvedContainerBox {
    let raw_size = style.size();
    let aspect_ratio = used_aspect_ratio(style.aspect_ratio());
    let box_sizing = style.box_sizing();
    let preferred_definite = if input.sizing_mode == SizingMode::IgnoreSizeStyles {
        Size::new(false, false)
    } else {
        preferred_size_definiteness(raw_size, input.parent_size, aspect_ratio)
    };
    let padding = resolve_padding(style.padding(), input.parent_size.width);
    let border = resolve_border(&style.border());
    let box_inset = box_inset_size(padding, border);
    let margin = auto_edges_to_zero(resolve_margins(style.margin(), input.parent_size.width));
    let (preferred, min, max) = if input.sizing_mode == SizingMode::IgnoreSizeStyles {
        (Size::NONE, Size::NONE, Size::NONE)
    } else {
        (
            resolve_quantitative_sizes(
                raw_size,
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
            ),
            resolve_quantitative_sizes(
                style.min_size(),
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
            ),
            resolve_quantitative_max_sizes(
                style.max_size(),
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
            ),
        )
    };
    let preferred = Size::new(
        preferred
            .width
            .map(|value| clamp_axis(value, min.width, max.width, box_inset.width)),
        preferred
            .height
            .map(|value| clamp_axis(value, min.height, max.height, box_inset.height)),
    );
    let outer = input.known_dimensions.or(preferred);
    let inner = Size::new(
        outer.width.map(|value| (value - box_inset.width).max(0.0)),
        outer
            .height
            .map(|value| (value - box_inset.height).max(0.0)),
    );
    let available_inner = Size::new(
        inner.width.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.width,
                    margin.horizontal_sum() + box_inset.width,
                )
            },
            AvailableSpace::Definite,
        ),
        inner.height.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.height,
                    margin.vertical_sum() + box_inset.height,
                )
            },
            AvailableSpace::Definite,
        ),
    );

    ResolvedContainerBox {
        preferred_definite,
        aspect_ratio,
        box_sizing,
        padding,
        border,
        box_inset,
        min,
        max,
        outer,
        inner,
        available_inner,
    }
}

#[inline]
pub(super) fn is_scroll_container(overflow: Point<Overflow>) -> bool {
    overflow.x.is_scrollable() || overflow.y.is_scrollable()
}

#[inline]
pub(super) fn accumulate_scrollable_overflow(
    content_size: &mut Size<f32>,
    location: Point<f32>,
    child_size: Size<f32>,
    child_content_size: Size<f32>,
    child_overflow: Point<Overflow>,
) {
    let reach = if is_scroll_container(child_overflow) {
        child_size
    } else {
        Size::new(
            child_size.width.max(child_content_size.width),
            child_size.height.max(child_content_size.height),
        )
    };
    content_size.width = content_size.width.max(location.x + reach.width);
    content_size.height = content_size.height.max(location.y + reach.height);
}

#[inline]
pub(super) fn own_scrollable_overflow<S: CoreStyle>(
    style: &S,
    border_box: Size<f32>,
    interior: Size<f32>,
) -> Size<f32> {
    if style.containment().contains(Contain::LAYOUT) && !is_scroll_container(style.overflow()) {
        border_box
    } else {
        interior
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use style_traits::values::specified::AllowedNumericType;
    use stylo::values::computed::Percentage;
    use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
    use stylo::values::generics::NonNegative;

    use super::*;

    macro_rules! assert_alignment_cases {
        ($normalize:ident; $(
            $name:literal: $flags:expr, $horizontal:literal, $rtl:literal => $expected:expr;
        )+) => {
            $(assert_eq!(
                $normalize($flags, $horizontal, $rtl), $expected, "{}", $name
            );)+
        };
    }

    #[test]
    fn percentage_and_calc_values_resolve_only_with_a_definite_basis() {
        let percent = LengthPercentage::new_percent(Percentage(0.5));
        assert_eq!(resolve_length_percentage(&percent, None), None);
        assert_eq!(resolve_length_percentage(&percent, Some(10.0)), Some(5.0));

        let mixed_calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(0.5))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(4.0))),
                ]
                .into(),
            ),
            AllowedNumericType::All,
        );
        assert_eq!(resolve_length_percentage(&mixed_calc, None), None);
        assert_eq!(
            resolve_length_percentage(&mixed_calc, Some(20.0)),
            Some(14.0)
        );

        let folded_calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(3.0))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(4.0))),
                ]
                .into(),
            ),
            AllowedNumericType::All,
        );
        assert_eq!(resolve_length_percentage(&folded_calc, None), Some(7.0));

        let size = StyleSize::LengthPercentage(NonNegative(percent.clone()));
        assert_eq!(resolve_style_size(&size, None), None);
        assert_eq!(resolve_style_size(&size, Some(30.0)), Some(15.0));
        assert_eq!(resolve_style_size(&StyleSize::Auto, Some(30.0)), None);
        let max = MaxSize::LengthPercentage(NonNegative(percent));
        assert_eq!(resolve_max_size(&max, Some(40.0)), Some(20.0));
        assert_eq!(resolve_max_size(&MaxSize::none(), Some(40.0)), None);
    }

    #[test]
    fn length_percentage_fast_path_matches_stylo_used_value_resolution() {
        let clamped_calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(0.25))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(-8.0))),
                ]
                .into(),
            ),
            AllowedNumericType::NonNegative,
        );
        let values = [
            LengthPercentage::new_length(Length::new(7.0)),
            LengthPercentage::new_percent(Percentage(0.25)),
            clamped_calc,
        ];

        for value in &values {
            for basis in [None, Some(0.0), Some(20.0), Some(80.0)] {
                let expected = value
                    .maybe_percentage_relative_to(basis.map(Length::new))
                    .map(|length| length.px().to_bits());
                let actual = resolve_length_percentage(value, basis).map(f32::to_bits);
                assert_eq!(actual, expected, "value={value:?}, basis={basis:?}");
            }
        }
    }

    #[test]
    fn margin_inset_and_gap_arms_cover_auto_and_normal() {
        let length = |px| LengthPercentage::new_length(Length::new(px));
        let percent = |fraction| LengthPercentage::new_percent(Percentage(fraction));

        assert_eq!(resolve_margin(&Margin::Auto, Some(10.0)), None);
        assert_eq!(
            resolve_margin(&Margin::LengthPercentage(length(3.0)), None),
            Some(3.0)
        );
        assert_eq!(resolve_inset(&Inset::Auto, Some(10.0)), None);
        assert_eq!(
            resolve_inset(&Inset::LengthPercentage(percent(0.1)), Some(50.0)),
            Some(5.0)
        );
        assert_eq!(
            resolve_gap_axis(&NonNegativeLengthPercentageOrNormal::Normal, Some(10.0)),
            0.0
        );
        let percent_gap =
            NonNegativeLengthPercentageOrNormal::LengthPercentage(NonNegative(percent(0.5)));
        assert_eq!(resolve_gap_axis(&percent_gap, Some(10.0)), 5.0);
    }

    #[test]
    fn item_align_flags_normalize_with_physical_and_fallback_handling() {
        use AlignFlags as A;

        assert_alignment_cases! { normalize_item_alignment;
            "auto": A::AUTO, true, false => None;
            "normal": A::NORMAL, false, false => None;
            "safe center": A::SAFE | A::CENTER, false, false => Some(A::CENTER);
            "last baseline fallback": A::LAST_BASELINE, false, false => Some(A::END);
            "physical left ltr": A::LEFT, true, false => Some(A::START);
            "physical left rtl": A::LEFT, true, true => Some(A::END);
            "physical right rtl": A::RIGHT, true, true => Some(A::START);
            "physical right vertical": A::RIGHT, false, false => Some(A::START);
            "self start": A::SELF_START, true, false => Some(A::START);
            "self end": A::SELF_END, false, false => Some(A::START);
        }
    }

    #[test]
    fn content_align_flags_normalize_with_physical_and_fallback_handling() {
        use AlignFlags as A;

        assert_alignment_cases! { normalize_content_alignment;
            "normal": A::NORMAL, false, false => None;
            "auto": A::AUTO, true, false => None;
            "space between": A::SPACE_BETWEEN, true, false => Some(A::SPACE_BETWEEN);
            "physical right ltr": A::RIGHT, true, false => Some(A::END);
            "physical right vertical": A::RIGHT, false, false => Some(A::START);
            "baseline fallback": A::BASELINE, false, false => Some(A::START);
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn ordered_item_stays_compact_on_64_bit_targets() {
        assert!(core::mem::size_of::<OrderedItem<usize>>() <= 24);
        assert!(core::mem::size_of::<OrderedItem<[usize; 2]>>() <= 32);
    }
}
