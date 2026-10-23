//! Post-flush computed-style views lending Stylo [`ComputedValues`] directly
//! to seldon without cloning its `Arc` or re-entering Stylo's runtime
//! borrow checker.

use seldon::geometry::Size;
use seldon::style::containment::effective_containment;
use seldon::style::{
    Contain, ContainIntrinsicSize, ContentVisibility, CoreStyle, Display, FlexboxStyle,
    GridLanesStyle, GridStyle, LinearStyle, LynxTextStyle, PositionProperty, RelativeStyle,
    TextContainerStyle, TextRunStyle,
};
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::WillChangeBits;

use super::committed_box;
use crate::tree::node::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    None,
    Contents,
    Flex,
    Grid,
    /// CSS Grid Level 3 `display: grid-lanes`: one axis carries the tracks,
    /// items stack along the other. Its children are grid items, so every
    /// grid-item rule outside layout applies to it as it does to
    /// [`DisplayMode::Grid`].
    GridLanes,
    Linear,
    Relative,
    /// A Lynx text element ([`Node::is_lynx_text`]): one flattened Lynx
    /// paragraph, laid out by [`seldon::text::block`] through
    /// [`crate::layout::text_block`].
    Text,
    /// A block container: inner display `flow` (`display: block`), whose
    /// content is laid out as a CSS inline formatting context by
    /// [`seldon::text::inline`] through [`crate::layout::inline_flow`].
    ///
    /// No author style produces it: the `lynx` grammar parses `block`,
    /// `flow` and `inline` only in the engine's own sheets. It is the display
    /// of the anonymous block box CSS wraps around loose text in a flex,
    /// grid, grid-lanes, linear or relative container (CSS 2.2 §9.2.1.1,
    /// css-flexbox-1 §4, css-grid-1 §6), a style the engine builds itself,
    /// and of the block containers of the HTML content the engine builds.
    ///
    /// An `inline` element maps here too, and lays its own content out when
    /// no inline formatting context took it in; inside a block container it
    /// is a non-atomic inline box of that container's context instead, and
    /// runs no layout of its own ([`crate::layout::inline_flow::is_inline_box`]).
    BlockFlow,
    Leaf,
}

/// The layout algorithm `node` runs: from its kind first, then from its
/// computed `display`.
///
/// `none` and `contents` act as computed on every element. Past them a Lynx
/// text element ([`Node::is_lynx_text`]) lays out as a Lynx paragraph whatever
/// its `display` says: that is what the element is, not a value of the
/// property, as native Lynx fixes a text's layout structurally
/// (`TextElement::ConvertToInlineElement`) and as an HTML `img` is replaced
/// content under any `display`. `contents` on a text has no Lynx behaviour to
/// follow; acting as computed is what keeps every reader of the style
/// consistent.
pub(crate) fn display_mode<T>(node: &Node<T>, display: Display) -> DisplayMode {
    mode_of(display, node.is_lynx_text())
}

fn mode_of(display: Display, lynx_text: bool) -> DisplayMode {
    match display {
        Display::None => DisplayMode::None,
        Display::Contents => DisplayMode::Contents,
        _ if lynx_text => DisplayMode::Text,
        Display::Flex => DisplayMode::Flex,
        Display::Grid => DisplayMode::Grid,
        Display::GridLanes => DisplayMode::GridLanes,
        Display::Linear => DisplayMode::Linear,
        Display::LynxRelative => DisplayMode::Relative,
        // Inner display `flow`: a block container, or an inline box that
        // lays its own content out because no inline formatting context
        // around it took it in (`crate::layout::inline_flow`).
        Display::Block | Display::Inline => DisplayMode::BlockFlow,
        // No `lynx` grammar path produces any other display today: the fork
        // blockifies every `display: contents` fixup (root and top layer) to
        // `flex`, never to Stylo's internal block/flow display. The panic
        // guards a Stylo value the layout cannot express, rather than letting
        // it silently become a leaf box.
        unsupported => panic!(
            "Bobcat does not support Stylo computed display {unsupported:?} \
             (raw={:#06x}, outside={:?}, inside={:?})",
            unsupported.to_u16(),
            unsupported.outside(),
            unsupported.inside(),
        ),
    }
}

fn is_root_element<T>(node: &Node<T>) -> bool {
    node.flat_parent().is_none_or(Node::is_document)
}

pub(crate) fn generates_no_box(style: &ComputedValues) -> bool {
    style.get_display().is_contents()
}

/// css-contain-2 §4: whether this element skips its contents.
///
/// `hidden` always does. `auto` does whenever the element is not *relevant to
/// the user* — the per-element bit
/// [`crate::layout::relevance`] owns, which the rendering update determines
/// against the region the paint walk's encode window admits. An `auto`
/// element no rendering update has reached yet reads as skipped, matching the
/// spec's "determined in the next rendering update".
///
/// A `display: contents` element generates no box, so it is never contained,
/// never skipped, and never a relayout boundary, whatever its
/// `content-visibility` computes to.
pub(crate) fn skips_contents<T>(node: &Node<T>, style: &ComputedValues) -> bool {
    if generates_no_box(style) {
        return false;
    }
    match *style.get_content_visibility() {
        ContentVisibility::Visible => false,
        ContentVisibility::Hidden => true,
        ContentVisibility::Auto => node.arenas().relevance(node.id()).skips(),
    }
}

pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    let box_style = style.get_box();
    let effects = style.get_effects();
    // filter-effects-1 §2.1 and filter-effects-2 §2.1 word this the same way:
    // a non-`none` value creates both a stacking context and a containing
    // block for absolutely and fixed positioned descendants, unless the
    // element it applies to is a document root element.
    let filters = !effects.filter.0.is_empty() || !effects.backdrop_filter.0.is_empty();
    !box_style.transform.0.is_empty()
        // web-animations-1: a current or in-effect transform animation acts
        // as `will-change: transform`, whatever the committed value.
        || node.animates_transform()
        || !matches!(
            box_style.perspective,
            stylo::values::generics::box_::Perspective::None
        )
        || !matches!(box_style.offset_path, OffsetPath::None)
        || box_style.will_change.bits.intersects(
            WillChangeBits::TRANSFORM | WillChangeBits::PERSPECTIVE | WillChangeBits::CONTAIN,
        )
        || (box_style
            .will_change
            .bits
            .intersects(WillChangeBits::FIXPOS_CB_NON_SVG)
            && !is_root_element(node))
        || effective_containment(
            *style.get_contain(),
            *style.get_content_visibility(),
            skips_contents(node, style),
            *style.get_container_type(),
        )
        .intersects(Contain::LAYOUT | Contain::PAINT)
        || (filters && !is_root_element(node))
}

pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    *style.get_box().get_position() != PositionProperty::Static
        // css-position-4 §3.1 computes a top-layer element's position to
        // `absolute` when it is neither `absolute` nor `fixed`. Stylo's
        // adjuster does so only for a style whose `-servo-top-layer` is
        // `auto`, which a UA rule has to declare: the embedder's UA sheet
        // does for the elements it puts in the layer (the `html` module's
        // `dialog:modal`, `:popover-open` and `::backdrop`), and for those
        // the position test above already answers. The layer itself is
        // generic — membership is
        // the embedder's call and needs no matching rule — so membership
        // stays the truth here for an element no such rule reaches
        // (`tree::top_layer`).
        || node.arenas().top_layer().places_against_viewport(node.id())
        || style
            .get_box()
            .will_change
            .bits
            .intersects(WillChangeBits::POSITION)
        || establishes_fixed_containing_block(node, style)
}

pub(crate) fn box_parent<T>(node: &Node<T>) -> Option<&Node<T>> {
    let mut current = node.flat_parent()?;
    loop {
        let style = StyleView::try_of(current)?;
        if !generates_no_box(style.values()) {
            return Some(current);
        }
        current = current.flat_parent()?;
    }
}

pub(crate) fn resolve_position<T>(node: &Node<T>, style: &ComputedValues) -> PositionProperty {
    // css-position-4 §3.1: a top-layer element's containing block, and its
    // `::backdrop`'s, is the initial one whatever the ancestors establish
    // (`tree::top_layer`). Stylo's adjuster already computed the position
    // to `absolute` or `fixed`.
    if node.arenas().top_layer().places_against_viewport(node.id()) {
        return PositionProperty::Fixed;
    }
    let parent_establishes = |fixed: bool| {
        box_parent(node).is_some_and(|parent| {
            StyleView::try_of(parent).is_some_and(|parent_style| {
                if fixed {
                    establishes_fixed_containing_block(parent, parent_style.values())
                } else {
                    establishes_absolute_containing_block(parent, parent_style.values())
                }
            })
        })
    };
    match *style.get_box().get_position() {
        computed @ (PositionProperty::Static
        | PositionProperty::Relative
        | PositionProperty::Sticky) => computed,
        PositionProperty::Absolute => {
            if parent_establishes(false) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
        PositionProperty::Fixed => {
            if parent_establishes(true) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
    }
}

/// The element style view seldon reads: a node handle for the
/// parent-dependent position lowering plus its post-flush computed values.
pub(crate) struct StyleView<'dom, T> {
    node: &'dom Node<T>,
    style: &'dom ComputedValues,
}

impl<T> std::fmt::Debug for StyleView<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("StyleView")
            .field(&self.node.id())
            .finish()
    }
}

impl<'dom, T> StyleView<'dom, T> {
    pub(crate) fn try_of(node: &'dom Node<T>) -> Option<Self> {
        Some(Self {
            node,
            style: node.layout_computed_style()?,
        })
    }

    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        Self::try_of(node).unwrap_or(Self {
            node,
            style: &super::ANONYMOUS_STYLE,
        })
    }

    /// `node` seen through another of its styles: a position option
    /// (css-anchor-position-1 §6.1) cascaded for it. Everything node-bound —
    /// the position lowering, containment, relevance — still reads `node`.
    pub(crate) const fn with_values(node: &'dom Node<T>, style: &'dom ComputedValues) -> Self {
        Self { node, style }
    }

    /// The computed values, borrowed for as long as the view's own borrow
    /// of the document, so a walk can keep them past the view.
    pub(crate) fn values(&self) -> &'dom ComputedValues {
        self.style
    }

    /// The layout algorithm the viewed node runs ([`display_mode`]).
    pub(crate) fn display_mode(&self) -> DisplayMode {
        display_mode(self.node, *self.style.get_display())
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn computed_values(&self) -> &ComputedValues {
        self.values()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, self.values())
    }

    /// The host-owned half of css-contain-2 §4: seldon reads
    /// `content-visibility: hidden` off computed style on its own, but
    /// `auto`'s relevance is a fact only a rendering update can establish, so
    /// it comes from this document's own per-element bit.
    ///
    /// [`CoreStyle::containment`] needs no override beside this one: its
    /// default already folds `self.skips_contents()` through
    /// `effective_containment`, which is where the `SIZE` bit a skipped
    /// `auto` box gains comes from.
    fn skips_contents(&self) -> bool {
        skips_contents(self.node, self.values())
    }

    /// The decoded dimensions of replaced content, the same ones
    /// `compute_leaf_layout` is handed below. A container reads them to settle
    /// css-grid-1 §6.2 `normal` self-alignment, which it has to do before it
    /// measures the item. A non-replaced node carries `NaturalSize::NONE`.
    fn natural_size(&self) -> Size<Option<f32>> {
        self.node.natural_size().dimensions()
    }

    /// css-sizing-4 §5.2.1 plus csswg-drafts#8407, which are both host-side
    /// for the same reason `skips_contents` is: the effective value depends on
    /// `content-visibility` and on a per-element size only a rendering update
    /// can have recorded. `seldon` reads whatever comes back exactly as it
    /// reads a computed value — `AutoLength(l)` as `l`, `AutoNone` as no
    /// explicit size — so the substitution needs no engine vocabulary of its
    /// own. See [`crate::layout::committed_box`].
    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        committed_box::contain_intrinsic_width(self.node, self.values())
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        committed_box::contain_intrinsic_height(self.node, self.values())
    }

    /// [`reads_percentage_basis`], as the node's own layout style answered
    /// it at its last harvest ([`Node::layout_reads_percentage_basis`]); a
    /// position option's style ([`Self::with_values`]) is evaluated here.
    #[inline]
    fn reads_percentage_basis(&self) -> bool {
        if self.node.views_own_layout_style(self.style) {
            self.node.layout_reads_percentage_basis()
        } else {
            reads_percentage_basis(self.node, self.style)
        }
    }
}

/// Whether laying `node` out under `style` reads the percentage basis its
/// parent hands it: through its own box model
/// ([`seldon::style::box_model_reads_percentage_basis`]), or because it
/// establishes a paragraph, which reads the basis beyond its box model — as
/// the width its lines fall back to, and as the basis of its atoms and inline
/// boxes (`inline_flow`, `text_block`).
///
/// Reads nothing of `node` but its kind ([`Node::is_lynx_text`]), which is
/// what lets the harvest store the answer as a node bit.
pub(crate) fn reads_percentage_basis<T>(node: &Node<T>, style: &ComputedValues) -> bool {
    super::paragraph::establishes_paragraph(display_mode(node, *style.get_display()))
        || seldon::style::box_model_reads_percentage_basis(&StyleView::with_values(node, style))
}

// Every algorithm accessor reads computed values, including registered
// paragraph-limit custom properties, through the style protocol defaults.
impl<T> FlexboxStyle for StyleView<'_, T> {}

impl<T> GridStyle for StyleView<'_, T> {}

impl<T> GridLanesStyle for StyleView<'_, T> {}

impl<T> LinearStyle for StyleView<'_, T> {}

impl<T> TextContainerStyle for StyleView<'_, T> {}

impl<T> LynxTextStyle for StyleView<'_, T> {}

impl<T> RelativeStyle for StyleView<'_, T> {}

/// Paragraph-level reads off bare computed values, for the callers that hold
/// no node: the two registered limits and `tail-color-convert`.
struct ParagraphStyle<'a>(&'a ComputedValues);

impl CoreStyle for ParagraphStyle<'_> {
    fn computed_values(&self) -> &ComputedValues {
        self.0
    }
}

impl TextContainerStyle for ParagraphStyle<'_> {}

impl LynxTextStyle for ParagraphStyle<'_> {}

/// Stylo gives custom-property changes repaint damage. Compare the effective
/// values the paragraph actually consumes to account for their layout
/// dependency: the two limits.
pub(crate) fn paragraph_limits_changed(old: &ComputedValues, new: &ComputedValues) -> bool {
    if old.custom_properties().non_inherited == new.custom_properties().non_inherited {
        return false;
    }

    let (old, new) = (ParagraphStyle(old), ParagraphStyle(new));
    old.text_maxline() != new.text_maxline() || old.text_maxlength() != new.text_maxlength()
}

/// Whether this paragraph's `tail-color-convert` is on, so its truncation
/// marker takes the establishing element's colour rather than the colour of
/// the run the cut landed in.
pub(crate) fn converts_tail_color(style: &ComputedValues) -> bool {
    ParagraphStyle(style).tail_color_convert()
}

/// The style one shaped run carries: everything Parley resolves per *run* —
/// the font family, size, weight, style, variations, features, `line-height`
/// and `letter-spacing`.
///
/// The paragraph's `StyleView` reads its establishing element; each run reads
/// its own innermost element so nested text keeps its font and line height.
pub(crate) struct TextRunView<'dom> {
    run: &'dom ComputedValues,
}

impl std::fmt::Debug for TextRunView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TextRunView")
    }
}

impl<'dom> TextRunView<'dom> {
    pub(crate) const fn from_values(run: &'dom ComputedValues) -> Self {
        Self { run }
    }

    /// The run style for `node`'s characters — the style of the innermost
    /// inline box containing them, which for a lone text child is its flat
    /// parent.
    pub(crate) fn of<T>(node: &'dom Node<T>) -> Self {
        Self {
            run: inline_style_of(node),
        }
    }
}

impl TextRunStyle for TextRunView<'_> {
    fn computed_text_values(&self) -> Option<&ComputedValues> {
        Some(self.run)
    }
}

pub(crate) fn inline_style_of<T>(node: &Node<T>) -> &ComputedValues {
    debug_assert!(node.is_text_node(), "text style requires a text node");
    node.flat_parent()
        .and_then(Node::layout_computed_style)
        .unwrap_or(&super::ANONYMOUS_STYLE)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::mem::size_of;

    use seldon::style::Display;

    use super::{DisplayMode, StyleView, TextRunView, mode_of};

    #[test]
    fn supported_lynx_displays_map_to_layout_modes() {
        assert_eq!(mode_of(Display::None, false), DisplayMode::None);
        assert_eq!(mode_of(Display::Contents, false), DisplayMode::Contents);
        assert_eq!(mode_of(Display::Flex, false), DisplayMode::Flex);
        assert_eq!(mode_of(Display::Grid, false), DisplayMode::Grid);
        assert_eq!(mode_of(Display::GridLanes, false), DisplayMode::GridLanes);
        assert_eq!(mode_of(Display::Linear, false), DisplayMode::Linear);
        assert_eq!(mode_of(Display::LynxRelative, false), DisplayMode::Relative);
        assert_eq!(mode_of(Display::Block, false), DisplayMode::BlockFlow);
        assert_eq!(mode_of(Display::Inline, false), DisplayMode::BlockFlow);
    }

    #[test]
    fn a_lynx_text_element_is_a_paragraph_under_every_display_but_none_and_contents() {
        assert_eq!(mode_of(Display::None, true), DisplayMode::None);
        assert_eq!(mode_of(Display::Contents, true), DisplayMode::Contents);
        for display in [
            Display::Flex,
            Display::Grid,
            Display::GridLanes,
            Display::Linear,
            Display::LynxRelative,
            Display::Block,
            Display::Inline,
        ] {
            assert_eq!(mode_of(display, true), DisplayMode::Text, "{display:?}");
        }
    }

    #[test]
    fn post_flush_style_views_stay_within_their_expected_footprint() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<StyleView<'static, ()>>(), 2 * word);
        assert_eq!(size_of::<TextRunView<'static>>(), word);
    }
}
