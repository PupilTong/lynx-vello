//! Stylo-computed-value resolution helpers shared by layout entry points.
//!
//! Algorithms work in f32 CSS pixels internally; these helpers lower stylo's
//! self-resolving computed values (`LengthPercentage` resolves its own
//! `calc()` trees) into that vocabulary once per pass. Percentage-carrying
//! values stay unresolved (`None`) while their basis is indefinite;
//! length-only `calc()` folds to a length at computed-value time and always
//! resolves (a documented behavior delta of the stylo vocabulary swap).

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
use crate::tree::{AvailableSpace, LayoutInput, SizingMode};

/// Interprets one alignment property's [`AlignFlags`] as item
/// self-alignment, yielding the canonical keyword subset the Flexbox and
/// Grid algorithms compare against (`START`/`END`/`FLEX_START`/`FLEX_END`/
/// `CENTER`/`BASELINE`/`STRETCH`).
///
/// `None` means `auto`/`normal` — the caller applies its contextual default.
/// Normalization policy (design amendment F): the engine interprets the
/// flags it understands; `SAFE`/`UNSAFE` are stripped by
/// [`AlignFlags::value`] (safe fallback ignored, as before the vocabulary
/// swap); last-baseline uses its specified fallback (the end edge, CSS Box
/// Alignment §4.2); the physical `LEFT`/`RIGHT` keywords map through the
/// alignment container's inline direction where the aligned axis is the
/// horizontal one (`inline_axis`) and fall back to start otherwise;
/// `self-start`/`self-end` (unreachable from the lynx grammar) and unknown
/// fabricated values fall back to start rather than crashing, because
/// cascade-less hosts may fabricate flag values.
pub(super) fn normalize_item_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignFlags> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        None
    } else if value == AlignFlags::LAST_BASELINE {
        // Last-baseline sharing is not implemented; its specified fallback
        // alignment is the end edge (CSS Box Alignment §4.2).
        Some(AlignFlags::END)
    } else if value == AlignFlags::START
        || value == AlignFlags::END
        || value == AlignFlags::FLEX_START
        || value == AlignFlags::FLEX_END
        || value == AlignFlags::CENTER
        || value == AlignFlags::BASELINE
        || value == AlignFlags::STRETCH
    {
        Some(value)
    } else if value == AlignFlags::LEFT && inline_axis {
        Some(if rtl {
            AlignFlags::END
        } else {
            AlignFlags::START
        })
    } else if value == AlignFlags::RIGHT && inline_axis {
        Some(if rtl {
            AlignFlags::START
        } else {
            AlignFlags::END
        })
    } else {
        Some(AlignFlags::START)
    }
}

/// Interprets one alignment property's [`AlignFlags`] as content
/// distribution, yielding the canonical keyword subset the Flexbox and Grid
/// algorithms compare against (`START`/`END`/`FLEX_START`/`FLEX_END`/
/// `CENTER`/`STRETCH`/`SPACE_BETWEEN`/`SPACE_AROUND`/`SPACE_EVENLY`).
///
/// `None` means `normal`; the caller applies the property's contextual
/// default. The flag policy matches [`normalize_item_alignment`]; baseline
/// content-alignment (unimplemented) and unknown fabricated values fall back
/// to their specified fallback: start.
pub(super) fn normalize_content_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignFlags> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        None
    } else if value == AlignFlags::START
        || value == AlignFlags::END
        || value == AlignFlags::FLEX_START
        || value == AlignFlags::FLEX_END
        || value == AlignFlags::CENTER
        || value == AlignFlags::STRETCH
        || value == AlignFlags::SPACE_BETWEEN
        || value == AlignFlags::SPACE_AROUND
        || value == AlignFlags::SPACE_EVENLY
    {
        Some(value)
    } else if value == AlignFlags::LEFT && inline_axis {
        Some(if rtl {
            AlignFlags::END
        } else {
            AlignFlags::START
        })
    } else if value == AlignFlags::RIGHT && inline_axis {
        Some(if rtl {
            AlignFlags::START
        } else {
            AlignFlags::END
        })
    } else {
        Some(AlignFlags::START)
    }
}

/// Node handle and order-modified paint index shared by formatting
/// algorithm scratch.
#[derive(Debug, Clone, Copy)]
pub(super) struct ItemKey<N> {
    pub(super) node: N,
    pub(super) layout_order: u32,
}

/// Algorithm-neutral ordering data collected before formatting-context item
/// classification. The field order intentionally packs this to 24 bytes on
/// 64-bit targets with a one-word handle (32 with a two-word handle); each
/// algorithm keeps one record per generated child.
#[derive(Debug, Clone, Copy)]
pub(super) struct OrderedItem<N> {
    pub(super) node: N,
    pub(super) document_index: usize,
    pub(super) css_order: i32,
    pub(super) layout_order: u32,
}

impl<N: Copy> OrderedItem<N> {
    /// Materializes the compact identity copied into algorithm-specific
    /// scratch after ordering is complete.
    #[inline]
    pub(super) const fn key(self) -> ItemKey<N> {
        ItemKey {
            node: self.node,
            layout_order: self.layout_order,
        }
    }
}

/// Access to the common ordering record retained by pre-layout item scratch.
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

/// Sorts in-flow items by `order` when needed and assigns one contiguous
/// order-modified paint sequence across in-flow and out-of-flow siblings.
///
/// Out-of-flow children contribute the initial `order` value (`0`) because
/// they are not formatting-context items. Both input slices must be in source
/// order on entry; only `in_flow` is reordered.
pub(super) fn sort_and_assign_layout_order<N, Item: PendingLayoutItem<N>>(
    in_flow: &mut [Item],
    absolute: &mut [Item],
) {
    let has_modified_order = in_flow.iter().any(|item| item.ordered().css_order != 0);
    if has_modified_order {
        in_flow.sort_unstable_by_key(|item| {
            let ordered = item.ordered();
            (ordered.css_order, ordered.document_index)
        });
        let mut paint_keys = Vec::with_capacity(in_flow.len() + absolute.len());
        paint_keys.extend(in_flow.iter().enumerate().map(|(index, item)| {
            let ordered = item.ordered();
            (ordered.css_order, ordered.document_index, false, index)
        }));
        paint_keys.extend(
            absolute
                .iter()
                .enumerate()
                .map(|(index, item)| (0, item.ordered().document_index, true, index)),
        );
        paint_keys.sort_unstable_by_key(|&(order, document, _, _)| (order, document));
        for (layout_order, &(_, _, is_absolute, index)) in paint_keys.iter().enumerate() {
            let layout_order = u32::try_from(layout_order).unwrap_or(u32::MAX);
            if is_absolute {
                absolute[index].ordered_mut().layout_order = layout_order;
            } else {
                in_flow[index].ordered_mut().layout_order = layout_order;
            }
        }
        return;
    }

    let (mut in_flow_index, mut absolute_index, mut layout_order) = (0, 0, 0_u32);
    while in_flow_index < in_flow.len() || absolute_index < absolute.len() {
        let take_in_flow = absolute_index == absolute.len()
            || (in_flow_index < in_flow.len()
                && in_flow[in_flow_index].ordered().document_index
                    < absolute[absolute_index].ordered().document_index);
        if take_in_flow {
            in_flow[in_flow_index].ordered_mut().layout_order = layout_order;
            in_flow_index += 1;
        } else {
            absolute[absolute_index].ordered_mut().layout_order = layout_order;
            absolute_index += 1;
        }
        layout_order = layout_order.saturating_add(1);
    }
}

/// Box classification inputs and resolved values common to layout items.
///
/// This is a short-lived resolver result. Each algorithm destructures it into
/// its own flat hot scratch so shared code does not constrain data layout.
/// Raw stylo values needed by algorithm-specific classification are returned
/// beside their resolved forms as borrows of the style view, so the resolver
/// never clones a computed value; the whole struct is `Copy`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedItemBox<'a> {
    pub(super) raw_size: Size<&'a StyleSize>,
    pub(super) raw_min_size: Size<&'a StyleSize>,
    pub(super) raw_max_size: Size<&'a MaxSize>,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) box_sizing: box_sizing::T,
    pub(super) overflow: Point<Overflow>,
    pub(super) preferred_size: Size<Option<f32>>,
    pub(super) min_size: Size<Option<f32>>,
    pub(super) max_size: Size<Option<f32>>,
    pub(super) margin: Edges<f32>,
    pub(super) margin_auto: Edges<bool>,
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) inset: Edges<Option<f32>>,
}

/// Algorithm-neutral resolved container box and sizing constraints.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedContainerBox {
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

/// Resolves a non-auto length-percentage against an optional percentage
/// basis.
///
/// Percentage-carrying values (including `calc()` trees that survive
/// computed-value folding with a percentage) remain unresolved when their
/// basis is indefinite. Absolute lengths never need a basis.
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

/// Resolves one margin edge, retaining `auto` as `None`.
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

/// Resolves one inset edge, retaining `auto` as `None`.
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

/// Resolves a quantitative preferred/minimum sizing value.
///
/// `auto`, the treated-as-auto keywords (bare `fit-content`, `stretch`,
/// `-webkit-fill-available`; behavior delta of the vocabulary swap), and the
/// intrinsic keywords intentionally remain unresolved here — intrinsic
/// keywords require content-contribution probes.
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

/// Resolves a quantitative maximum sizing value (`none` behaves as `auto`).
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
    Size::new(
        resolve_style_size(value.width, basis.width),
        resolve_style_size(value.height, basis.height),
    )
}

#[inline]
pub(super) fn resolve_max_sizes(
    value: Size<&MaxSize>,
    basis: Size<Option<f32>>,
) -> Size<Option<f32>> {
    Size::new(
        resolve_max_size(value.width, basis.width),
        resolve_max_size(value.height, basis.height),
    )
}

/// Resolves padding edges. CSS resolves percentages on all four physical
/// sides against the containing block's width.
#[inline]
pub(super) fn resolve_padding(
    value: Edges<&NonNegativeLengthPercentage>,
    inline_basis: Option<f32>,
) -> Edges<f32> {
    Edges {
        left: resolve_padding_edge(value.left, inline_basis),
        right: resolve_padding_edge(value.right, inline_basis),
        top: resolve_padding_edge(value.top, inline_basis),
        bottom: resolve_padding_edge(value.bottom, inline_basis),
    }
}

#[inline]
fn resolve_padding_edge(value: &NonNegativeLengthPercentage, inline_basis: Option<f32>) -> f32 {
    resolve_length_percentage(&value.0, inline_basis)
        .unwrap_or(0.0)
        .max(0.0)
}

/// Lowers used border widths. Computed border widths are absolute (`Au`) and
/// never depend on a percentage basis; the host supplies used widths (zero
/// when the border style is `none`), so this is a plain unit conversion.
#[inline]
pub(super) fn resolve_border(value: &Edges<BorderSideWidth>) -> Edges<f32> {
    Edges {
        left: checked(value.left.0.to_f32_px()).max(0.0),
        right: checked(value.right.0.to_f32_px()).max(0.0),
        top: checked(value.top.0.to_f32_px()).max(0.0),
        bottom: checked(value.bottom.0.to_f32_px()).max(0.0),
    }
}

/// Resolves margins while retaining `auto` as `None`.
#[inline]
pub(super) fn resolve_margins(
    value: Edges<&Margin>,
    inline_basis: Option<f32>,
) -> Edges<Option<f32>> {
    Edges {
        left: resolve_margin(value.left, inline_basis),
        right: resolve_margin(value.right, inline_basis),
        top: resolve_margin(value.top, inline_basis),
        bottom: resolve_margin(value.bottom, inline_basis),
    }
}

#[inline]
pub(super) fn auto_edges_to_zero(value: Edges<Option<f32>>) -> Edges<f32> {
    value.map(|side| side.unwrap_or(0.0))
}

/// Resolves physical insets. Horizontal percentages use the containing
/// block width; vertical percentages use its height.
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

/// Converts quantitative content-box sizing properties to border-box sizes.
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

/// Fills the ratio-dependent axis when exactly one axis is definite.
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

/// Converts the computed `aspect-ratio` to the engine's used `width / height`
/// value; degenerate ratios behave as `auto` per CSS Sizing 4.
#[inline]
pub(super) fn used_aspect_ratio(value: AspectRatio) -> Option<f32> {
    match value.ratio {
        PreferredRatio::None => None,
        PreferredRatio::Ratio(ratio) => (!ratio.is_degenerate()).then(|| ratio.0.0 / ratio.1.0),
    }
}

/// Whether one preferred-size axis establishes a definite percentage basis.
/// Length-only `calc()` folds to a length at computed-value time and is
/// definite without a basis (behavior delta of the vocabulary swap).
#[inline]
fn style_size_is_definite(value: &StyleSize, parent_basis: Option<f32>) -> bool {
    match value {
        StyleSize::LengthPercentage(lp) => !lp.0.has_percentage() || parent_basis.is_some(),
        StyleSize::Auto
        | StyleSize::MinContent
        | StyleSize::MaxContent
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable
        | StyleSize::FitContentFunction(_) => false,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

/// Returns which preferred-size axes establish a definite percentage basis.
/// A preferred aspect ratio (the [`used_aspect_ratio`] value) transfers
/// definiteness across axes just as it transfers the resolved preferred size.
#[inline]
pub(super) fn preferred_size_definiteness(
    size: Size<&StyleSize>,
    parent_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<bool> {
    let mut definite = Size::new(
        style_size_is_definite(size.width, parent_size.width),
        style_size_is_definite(size.height, parent_size.height),
    );
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
    // CSS gives the minimum precedence when max < min.
    value
        .min(max.unwrap_or(f32::INFINITY))
        .max(min.unwrap_or(0.0))
}

/// Resolves relative-position insets to a physical visual offset.
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

/// Size consumed by padding and borders. Lynx scrollbars are overlay-only,
/// so no scrollbar space ever joins this inset.
#[inline]
pub(super) fn box_inset_size(padding: Edges<f32>, border: Edges<f32>) -> Size<f32> {
    Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    )
}

/// Resolves preferred/min quantitative sizes into border-box values.
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

/// Resolves max quantitative sizes into border-box values.
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

/// Applies CSS min/max precedence and a border-box floor on one axis.
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

/// Resolves one non-negative gap axis (`normal` resolves to zero).
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

/// Resolves non-negative row/column gaps against their respective bases.
#[inline]
pub(super) fn resolve_gap(
    value: Size<&NonNegativeLengthPercentageOrNormal>,
    basis: Size<Option<f32>>,
) -> Size<f32> {
    Size::new(
        resolve_gap_axis(value.width, basis.width),
        resolve_gap_axis(value.height, basis.height),
    )
}

/// Resolves the algorithm-neutral box values of one layout item.
#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a large resolver result and copy chain in release LLVM IR"
)]
pub(super) fn resolve_item_box(
    style: &impl CoreStyle,
    percentage_basis: Size<Option<f32>>,
) -> ResolvedItemBox<'_> {
    resolve_item_box_with_bases(style, percentage_basis, percentage_basis.width)
}

/// Resolves an item's box when sizing percentages and physical edge
/// percentages have different bases.
///
/// Relative layout uses the definite parent content size for child sizing,
/// while margins/padding resolve against the available parent width. Flex
/// and Grid use [`resolve_item_box`], where both bases are identical.
#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a large resolver result and copy chain in release LLVM IR"
)]
pub(super) fn resolve_item_box_with_bases(
    style: &impl CoreStyle,
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
) -> ResolvedItemBox<'_> {
    let raw_size = style.size();
    let raw_min_size = style.min_size();
    let raw_max_size = style.max_size();
    let aspect_ratio = used_aspect_ratio(style.aspect_ratio());
    let box_sizing = style.box_sizing();
    let overflow = style.overflow();
    let padding_value = style.padding();
    let border_value = style.border();
    let inset_value = style.inset();
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

    ResolvedItemBox {
        raw_size,
        raw_min_size,
        raw_max_size,
        aspect_ratio,
        box_sizing,
        overflow,
        preferred_size,
        min_size,
        max_size,
        margin: auto_edges_to_zero(optional_margin),
        margin_auto: margin_value.map(Margin::is_auto),
        padding,
        border,
        inset: resolve_insets(inset_value, size_percentage_basis),
    }
}

/// Resolves the common container box before algorithm-specific sizing.
#[inline]
pub(super) fn resolve_container_box(
    style: &impl CoreStyle,
    input: LayoutInput,
) -> ResolvedContainerBox {
    let padding = resolve_padding(style.padding(), input.parent_size.width);
    let border = resolve_border(&style.border());
    let box_inset = box_inset_size(padding, border);
    let margin = auto_edges_to_zero(resolve_margins(style.margin(), input.parent_size.width));
    let (preferred, min, max) = if input.sizing_mode == SizingMode::ContentSize {
        (Size::NONE, Size::NONE, Size::NONE)
    } else {
        let aspect_ratio = used_aspect_ratio(style.aspect_ratio());
        let box_sizing = style.box_sizing();
        (
            resolve_quantitative_sizes(
                style.size(),
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

/// Whether these `overflow` values make the box a **scroll container** per
/// [CSS Overflow 3 §3.1][overflow-3-3.1]: any axis whose value is one of the
/// *scrollable* values (`scroll`/`auto`/`hidden`) — i.e. any non-`visible`
/// axis. (Under the stylo `lynx` feature the only non-`visible` value is
/// `hidden`, which CSS Overflow 3 still classifies as a scroll container: it is
/// programmatically scrollable and clips.)
///
/// A scroll container **traps** its interior scrollable overflow
/// ([CSS Overflow 3 §3.3][overflow-3-3.3]: a descendant's scrollable-overflow
/// rectangle is "clipped to their overflow clip edge if overflow is not
/// visible"): it contributes only its border box to an ancestor's scrollable
/// overflow, while keeping its own `content_size` as its private scroll range
/// — see [`accumulate_scrollable_overflow`].
///
/// [overflow-3-3.1]: https://drafts.csswg.org/css-overflow-3/#overflow-properties
/// [overflow-3-3.3]: https://drafts.csswg.org/css-overflow-3/#scrollable
#[inline]
pub(super) fn is_scroll_container(overflow: Point<Overflow>) -> bool {
    overflow.x.is_scrollable() || overflow.y.is_scrollable()
}

/// Folds one child's scrollable-overflow contribution into the container's
/// running `content_size`, at the child's border-box `location` (container
/// border-box space), applying the [CSS Overflow 3 §3.3][overflow-3-3.3]
/// trapping rule.
///
/// A **scroll-container** child ([`is_scroll_container`]) contributes only its
/// border box (`child_size`): its own `content_size` is its private, trapped
/// scroll range and must never leak into an ancestor's scrollable overflow. Any
/// other child contributes the union of its border box and its own (already
/// trapping-aware) `content_size` — the standard transitive scrollable-overflow
/// propagation.
///
/// [overflow-3-3.3]: https://drafts.csswg.org/css-overflow-3/#scrollable
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

/// A container's **own** scrollable overflow (`content_size`), applying
/// [CSS Contain 2 §3.3 Layout Containment][contain-2-3.3].
///
/// A box with effective **layout containment** whose `overflow` is `visible`
/// (or `clip`) — i.e. **not** a scroll container — treats descendant overflow
/// as *ink* overflow (§3.3 item 3: "any overflow must be treated as ink
/// overflow"), which is excluded from the scrollable overflow region. It
/// therefore reports only its own border box, ignoring the accumulated
/// `interior` union.
///
/// A scroll container, or an uncontained box, reports the accumulated
/// `interior` union unchanged; for a scroll container that union is its real
/// scroll range (CSS Overflow 3), and the trapping toward *ancestors* happens
/// at their accumulation site instead (see [`accumulate_scrollable_overflow`]).
///
/// [contain-2-3.3]: https://drafts.csswg.org/css-contain-2/#containment-layout
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

        // Length-only calc() folds at computed-value time and resolves
        // without a basis (behavior delta of the vocabulary swap).
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
        assert_eq!(resolve_margin(&Margin::Auto, Some(10.0)), None);
        assert_eq!(
            resolve_margin(
                &Margin::LengthPercentage(LengthPercentage::new_length(Length::new(3.0))),
                None,
            ),
            Some(3.0)
        );
        assert_eq!(resolve_inset(&Inset::Auto, Some(10.0)), None);
        assert_eq!(
            resolve_inset(
                &Inset::LengthPercentage(LengthPercentage::new_percent(Percentage(0.1))),
                Some(50.0),
            ),
            Some(5.0)
        );
        assert_eq!(
            resolve_gap_axis(&NonNegativeLengthPercentageOrNormal::Normal, Some(10.0)),
            0.0
        );
        assert_eq!(
            resolve_gap_axis(
                &NonNegativeLengthPercentageOrNormal::LengthPercentage(NonNegative(
                    LengthPercentage::new_percent(Percentage(0.5)),
                )),
                Some(10.0),
            ),
            5.0
        );
    }

    #[test]
    fn item_align_flags_normalize_with_physical_and_fallback_handling() {
        assert_eq!(
            normalize_item_alignment(AlignFlags::AUTO, true, false),
            None
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::NORMAL, false, false),
            None
        );
        // The SAFE qualifier is stripped; the value nibble decides.
        assert_eq!(
            normalize_item_alignment(AlignFlags::SAFE | AlignFlags::CENTER, false, false),
            Some(AlignFlags::CENTER)
        );
        // Last baseline uses its specified fallback: the end edge.
        assert_eq!(
            normalize_item_alignment(AlignFlags::LAST_BASELINE, false, false),
            Some(AlignFlags::END)
        );
        // Physical keywords map through direction on the inline axis...
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, true, false),
            Some(AlignFlags::START)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, true, true),
            Some(AlignFlags::END)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::RIGHT, true, true),
            Some(AlignFlags::START)
        );
        // ...and fall back to start in the block axis.
        assert_eq!(
            normalize_item_alignment(AlignFlags::RIGHT, false, false),
            Some(AlignFlags::START)
        );
        // Values outside the engine's understood set fall back to start.
        assert_eq!(
            normalize_item_alignment(AlignFlags::SELF_START, true, false),
            Some(AlignFlags::START)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::SELF_END, false, false),
            Some(AlignFlags::START)
        );
    }

    #[test]
    fn content_align_flags_normalize_with_physical_and_fallback_handling() {
        assert_eq!(
            normalize_content_alignment(AlignFlags::NORMAL, false, false),
            None
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::AUTO, true, false),
            None
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::SPACE_BETWEEN, true, false),
            Some(AlignFlags::SPACE_BETWEEN)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::RIGHT, true, false),
            Some(AlignFlags::END)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::RIGHT, false, false),
            Some(AlignFlags::START)
        );
        // Baseline content alignment falls back to start.
        assert_eq!(
            normalize_content_alignment(AlignFlags::BASELINE, false, false),
            Some(AlignFlags::START)
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn ordered_item_stays_compact_on_64_bit_targets() {
        // One-word handles (a plain `&Node`) keep the historical 24-byte
        // packing; two-word handles (`(&Tree, index)`) pay one extra word.
        assert_eq!(core::mem::size_of::<OrderedItem<usize>>(), 24);
        assert_eq!(core::mem::size_of::<OrderedItem<[usize; 2]>>(), 32);
    }
}
