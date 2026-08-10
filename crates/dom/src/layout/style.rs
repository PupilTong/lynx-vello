//! Post-flush computed-style views lending Stylo [`ComputedValues`] directly
//! to hughie without cloning its `Arc` or re-entering Stylo's runtime
//! borrow checker.

use hughie::style::containment::effective_containment;
use hughie::style::{
    Contain, ContentVisibility, CoreStyle, Display, PositionProperty, TextContainerStyle,
    TextRunStyle,
};
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::{DisplayInside, DisplayOutside, WillChangeBits};

use crate::tree::node::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    None,
    Contents,
    Flow { inline: bool },
    Flex { inline: bool },
    Grid { inline: bool },
    Linear { inline: bool },
    Relative { inline: bool },
    Leaf,
}

impl DisplayMode {
    pub(crate) const fn is_none(self) -> bool {
        matches!(self, Self::None)
    }

    pub(crate) const fn is_contents(self) -> bool {
        matches!(self, Self::Contents)
    }

    pub(crate) const fn is_leaf(self) -> bool {
        matches!(self, Self::Leaf)
    }

    pub(crate) const fn is_inline(self) -> bool {
        matches!(
            self,
            Self::Flow { inline: true }
                | Self::Flex { inline: true }
                | Self::Grid { inline: true }
                | Self::Linear { inline: true }
                | Self::Relative { inline: true }
        )
    }

    pub(crate) const fn is_flow(self) -> bool {
        matches!(self, Self::Flow { .. })
    }

    pub(crate) const fn is_item_container(self) -> bool {
        matches!(self, Self::Flex { .. } | Self::Grid { .. })
    }
}

pub(crate) fn display_mode(display: Display) -> DisplayMode {
    if display.is_contents() {
        return DisplayMode::Contents;
    }
    if display.outside() == DisplayOutside::None {
        return DisplayMode::None;
    }
    let inline = display.outside() == DisplayOutside::Inline;
    match display.inside() {
        DisplayInside::None => DisplayMode::None,
        DisplayInside::Flex => DisplayMode::Flex { inline },
        DisplayInside::Grid => DisplayMode::Grid { inline },
        DisplayInside::LynxLinear => DisplayMode::Linear { inline },
        DisplayInside::LynxRelative => DisplayMode::Relative { inline },
        DisplayInside::Flow => DisplayMode::Flow { inline },
        DisplayInside::Contents => DisplayMode::Contents,
    }
}

fn is_root_element<T>(node: &Node<T>) -> bool {
    node.flat_parent().is_none_or(Node::is_document)
}

pub(crate) fn generates_no_box(style: &ComputedValues) -> bool {
    style.clone_display().is_contents()
}

pub(crate) fn skips_contents(style: &ComputedValues) -> bool {
    !generates_no_box(style) && style.clone_content_visibility() == ContentVisibility::Hidden
}

pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    let box_style = style.get_box();
    !box_style.transform.0.is_empty()
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
            style.clone_contain(),
            style.clone_content_visibility(),
            skips_contents(style),
        )
        .intersects(Contain::LAYOUT | Contain::PAINT)
        || (!style.get_effects().filter.0.is_empty() && !is_root_element(node))
}

pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    style.clone_position() != PositionProperty::Static
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
    match style.clone_position() {
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

/// The element style view hughie reads: a node handle for the
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

    pub(crate) const fn values(&self) -> &'dom ComputedValues {
        self.style
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn computed_values(&self) -> &ComputedValues {
        self.values()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, self.values())
    }
}

/// Whether a restyle changed anything Parley *shapes* from, and therefore
/// whether a descendant text node's retained layout has to be thrown away
/// rather than merely re-broken.
///
/// Two levels, the shape Stylo's own damage computation uses
/// (`properties.mako.rs`, `restyle_damage_rebuild_box`): a pointer comparison
/// on the two style structs that hold every shaping input, and — only when one
/// of those differs — a field comparison narrowed to the inputs themselves.
/// The second level is what earns the first its keep: `text-align`,
/// `text-indent`, `-webkit-text-stroke-*` and `color` share `InheritedText`
/// with `letter-spacing` and `word-break`, so a struct-granular answer alone
/// would still re-shape a paragraph whose only change was its alignment.
///
/// The field list mirrors [`crate::layout::TextLayout`]'s shaping inputs —
/// what `hughie`'s `translate_run_style` and `normalize_runs` read, and
/// nothing else. `direction` is deliberately absent: Parley is never told a
/// base direction (bidi comes from the text), so `direction` only selects the
/// alignment a commit re-applies anyway. Keeping this list in step with the
/// measurer is guarded in debug builds by the shaping fingerprint the retained
/// layout carries, which panics if a kept artifact outlived its inputs.
pub(crate) fn shaping_inputs_changed(old: &ComputedValues, new: &ComputedValues) -> bool {
    let (old_font, new_font) = (old.get_font(), new.get_font());
    let (old_text, new_text) = (old.get_inherited_text(), new.get_inherited_text());
    if std::ptr::eq(old_font, new_font) && std::ptr::eq(old_text, new_text) {
        return false;
    }
    (!std::ptr::eq(old_font, new_font)
        && (old_font.font_family != new_font.font_family
            || old_font.font_size != new_font.font_size
            || old_font.font_weight != new_font.font_weight
            || old_font.font_style != new_font.font_style
            || old_font.line_height != new_font.line_height
            || old_font.font_feature_settings != new_font.font_feature_settings
            || old_font.font_variation_settings != new_font.font_variation_settings))
        || (!std::ptr::eq(old_text, new_text)
            && (old_text.letter_spacing != new_text.letter_spacing
                || old_text.word_break != new_text.word_break
                || old_text.text_wrap_mode != new_text.text_wrap_mode
                || old_text.white_space_collapse != new_text.white_space_collapse))
}

impl<T> TextContainerStyle for StyleView<'_, T> {}

/// The style of the box that establishes a text node's formatting context.
///
/// Paragraph-level: the anonymous box contributes no geometry of its own, so
/// [`CoreStyle`] answers from the initial values, and everything Parley reads
/// per *paragraph* — `white-space`, `word-break`, `text-wrap`, `text-align`,
/// `text-indent` — comes from the inherited values of the establishing
/// element.
///
/// This is deliberately a different view from [`TextRunView`] even though both
/// resolve to the same element today. A text node is a single anonymous run
/// inside its parent, so the two roles coincide; in an inline formatting
/// context they do not — one paragraph spans many runs, and each run carries
/// the style of the innermost inline box its characters sit in. Keeping the
/// roles apart here is what lets the run side gain its own style source
/// without disturbing the paragraph side.
pub(crate) struct TextContainerView<'dom> {
    paragraph: &'dom ComputedValues,
}

impl std::fmt::Debug for TextContainerView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TextContainerView")
    }
}

impl<'dom> TextContainerView<'dom> {
    /// The paragraph style for `node`'s anonymous box — the style of the
    /// element that establishes its formatting context, which for a lone text
    /// child is its flat parent.
    pub(crate) fn of<T>(node: &'dom Node<T>) -> Self {
        Self {
            paragraph: inline_style_of(node),
        }
    }
}

impl CoreStyle for TextContainerView<'_> {
    fn computed_values(&self) -> &ComputedValues {
        &super::ANONYMOUS_STYLE
    }

    fn inherited_values(&self) -> &ComputedValues {
        self.paragraph
    }
}

impl TextContainerStyle for TextContainerView<'_> {}

/// The style one shaped run carries: everything Parley resolves per *run* —
/// the font family, size, weight, style, variations, features, `line-height`
/// and `letter-spacing`.
///
/// Today a text node is one run and its style is its parent's; see
/// [`TextContainerView`] for why the two views exist separately anyway.
pub(crate) struct TextRunView<'dom> {
    run: &'dom ComputedValues,
}

impl std::fmt::Debug for TextRunView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TextRunView")
    }
}

impl<'dom> TextRunView<'dom> {
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

fn inline_style_of<T>(node: &Node<T>) -> &ComputedValues {
    debug_assert!(node.is_text_node(), "text style requires a text node");
    node.flat_parent()
        .and_then(Node::layout_computed_style)
        .unwrap_or(&super::ANONYMOUS_STYLE)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::mem::size_of;

    use hughie::style::Display;
    use stylo::values::specified::box_::DisplayInside;

    use super::{DisplayMode, StyleView, TextContainerView, TextRunView, display_mode};

    #[test]
    fn supported_lynx_displays_map_to_layout_modes() {
        assert_eq!(display_mode(Display::None), DisplayMode::None);
        assert_eq!(display_mode(Display::Contents), DisplayMode::Contents);
        assert_eq!(
            display_mode(Display::Flex),
            DisplayMode::Flex { inline: false }
        );
        assert_eq!(
            display_mode(Display::Grid),
            DisplayMode::Grid { inline: false }
        );
        assert_eq!(
            display_mode(Display::Linear),
            DisplayMode::Linear { inline: false }
        );
        assert_eq!(
            display_mode(Display::LynxRelative),
            DisplayMode::Relative { inline: false }
        );
        assert_eq!(
            display_mode(Display::InlineFlex),
            DisplayMode::Flex { inline: true }
        );
        assert_eq!(
            display_mode(Display::InlineGrid),
            DisplayMode::Grid { inline: true }
        );
        assert_eq!(
            display_mode(Display::InlineLinear),
            DisplayMode::Linear { inline: true }
        );
        assert_eq!(
            display_mode(Display::InlineRelative),
            DisplayMode::Relative { inline: true }
        );
    }

    #[test]
    fn internal_block_flow_maps_to_the_paragraph_algorithm() {
        let flow = Display::Contents.equivalent_block_display(true);
        assert_eq!(flow.inside(), DisplayInside::Flow);
        assert_eq!(display_mode(flow), DisplayMode::Flow { inline: false });
    }

    #[test]
    fn post_flush_style_views_stay_within_their_expected_footprint() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<StyleView<'static, ()>>(), 2 * word);
        assert_eq!(size_of::<TextContainerView<'static>>(), word);
        assert_eq!(size_of::<TextRunView<'static>>(), word);
    }
}
