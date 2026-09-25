// Cards read against their own source, whose rendering this engine gets right.
// The golden beside this file is that rendering, so a failure is a change to
// judge — never a difference from another engine.
//
// Each entry names what the card tests, because that is what the golden is
// evidence of; a rendering nobody can restate is not a verified one.

verified! {
    // `var()` chained through a second custom property resolves to pink.
    basic_css_var => "basic-css-var",
    // `background-image: url(…)` from CSS, framed by `background-size: contain`.
    basic_css_asset_in_css => "basic-css-asset-in-css",
    // `.a.b` (0,2,0) beats `.wrapper view` (0,1,1) on the first box alone.
    basic_css_compound_selector => "basic-css-compound-selector",
    // The later class wins, and the gradient fills the letterforms themselves.
    basic_element_text_color => "basic-element-text-color",
    // `display: none` on a `<text>` still paints: the 2026-09-14 ruling.
    basic_element_text_display_none => "basic-element-text-display-none",
    // Class against inline text styling across four runs of a flex row.
    basic_element_text_dynamic_text_style_update => "basic-element-text-dynamic-text-style-update",
    // An `@font-face` family reaches shaping.
    basic_element_text_extra_font_family => "basic-element-text-extra-font-family",
    // `text-maxline` 1 / 2 / unset clamps the wrapped paragraph, and the
    // marker stays gated on `text-overflow` (2026-09-14 ruling).
    basic_element_text_maxline => "basic-element-text-maxline",
    // `text-maxlength` cuts by character, including CJK and a mid-word 200.
    basic_element_text_maxlength => "basic-element-text-maxlength",
    // A 22x22 `<image>` as an inline atom, its `margin-left` on the line.
    basic_element_text_nest_image => "basic-element-text-nest-image",
    // A `<view>` inside a `<text>` as an atomic inline box.
    basic_element_text_nest_view => "basic-element-text-nest-view",
    // `tail-color-convert` follows native: the marker wears the cut run.
    basic_element_text_tail_color_convert => "basic-element-text-tail-color-convert",
    // A gradient paints through the paragraph's own runs.
    basic_element_text_text_with_linear_gradient => "basic-element-text-text-with-linear-gradient",
    // A literal newline in a `raw-text` breaks the line.
    basic_element_text_with_new_line => "basic-element-text-with-new-line",
    // Digits, Latin and CJK break where `word-break` says they do.
    basic_element_text_word_break => "basic-element-text-word-break",
    // absolutely positioned children are skipped by the column linear flow
    basic_linear_absolute_not_in_flow => "basic-linear-absolute-not-in-flow",
    // a wrapping flex child inside a linear container, and the parent sized to it
    basic_linear_child_container_wrap => "basic-linear-child-container-wrap",
    // `align-items` as linear cross gravity, `height: auto` stretching
    basic_linear_column_align_items => "basic-linear-column-align-items",
    // per-item `linear-layout-gravity`: a deprecated longhand this engine drops, so every item falls back to cross-start
    basic_linear_column_container_items_layout_gravity => "basic-linear-column-container-items-layout-gravity",
    // `linear-gravity` centre values, dropped as deprecated, leaving start packing in both directions
    basic_linear_column_container_main_axis_graverty_center => "basic-linear-column-container-main-axis-graverty-center",
    // dropped `linear-gravity: start|end`; the vertical main axis leaves RTL nothing to show, as upstream's own shot also has it
    basic_linear_column_container_main_axis_graverty_start_end_with_direction_rtl => "basic-linear-column-container-main-axis-graverty-start-end-with-direction-rtl",
    // the same with `top|bottom`, and RTL again invisible in this geometry
    basic_linear_column_container_main_axis_graverty_top_bottom_with_direction_rtl => "basic-linear-column-container-main-axis-graverty-top-bottom-with-direction-rtl",
    // `justify-content: left|right` is outside Lynx's value set, so it is dropped and every column packs at start
    basic_linear_column_container_main_axis_justify_content_right_left_with_direction_rtl => "basic-linear-column-container-main-axis-justify-content-right-left-with-direction-rtl",
    // container-side `linear-cross-gravity`, dropped as deprecated, with `height: auto` stretching
    basic_linear_column_cross_gravity => "basic-linear-column-cross-gravity",
    // `linear-gravity: space-between`, dropped, leaving start packing with no gap
    basic_linear_graverty_space_between => "basic-linear-graverty-space-between",
    // standard `direction: rtl` reverses a column linear's cross axis
    basic_linear_orientation_vertical_with_direction => "basic-linear-orientation-vertical-with-direction",
    // per-item gravities in a row container, dropped, with `height: auto` items stretching
    basic_linear_row_container_items_layout_gravity => "basic-linear-row-container-items-layout-gravity",
    // dropped centre gravity on row and row-reverse: left packing and right packing with reversed order
    basic_linear_row_container_main_axis_graverty_center => "basic-linear-row-container-main-axis-graverty-center",
    // container `linear-cross-gravity` on column containers, dropped, with `width: auto` stretching
    basic_linear_row_cross_gravity => "basic-linear-row-cross-gravity",
    // Starlight applies `linear-weight-sum` only when it shrinks the distributable space
    basic_linear_sum_of_item_weight_larger_than_container_weight_sum => "basic-linear-sum-of-item-weight-larger-than-container-weight-sum",
    // a weight sum above the item total scales the free space and leaves the remainder unassigned
    basic_linear_weight_not_assign_full_free_space => "basic-linear-weight-not-assign-full-free-space",
    // a weight sum equal to the item total is a scale of one
    basic_linear_weight_sum_equal_to_item_weight => "basic-linear-weight-sum-equal-to-item-weight",
    // a zero weight sum disables the override and distribution falls back to the active weight
    basic_linear_weight_sum_is_zero => "basic-linear-weight-sum-is-zero",
    // the automatic minimum size does not come from the content size suggestion
    basic_flex_item_main_axis_content_based_min_size_not_from_content_size_suggestion => "basic-flex-item-main-axis-content-based-min-size-not-from-content-size-suggestion",
    // scaled shrink over unequal bases keeps the smaller item from collapsing
    basic_flex_item_not_shrink_to_zero => "basic-flex-item-not-shrink-to-zero",
    // two equal items shrink equally, and the overflowing child is clipped at the container
    basic_flex_item_shrink => "basic-flex-item-shrink",
    // `:root` matches the `<page>` element
    basic_style_root_selector => "basic-style-root-selector",
}
