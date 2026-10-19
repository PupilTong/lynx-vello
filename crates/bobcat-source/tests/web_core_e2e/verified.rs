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
    // Four `<text style='display:none'>`, two directly in a view and two nested in a `<text>`: all hidden, so nothing paints. `display: none` hides a `<text>` since #423, which reversed the 2026-09-14 ruling that it stays unsupported; web-core's screenshot is blank as well.
    basic_element_text_display_none => "basic-element-text-display-none",
    // Class against inline text styling across four runs of a flex row.
    basic_element_text_dynamic_text_style_update => "basic-element-text-dynamic-text-style-update",
    // An `@font-face` family reaches shaping.
    basic_element_text_extra_font_family => "basic-element-text-extra-font-family",
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
    // two equal items shrink equally; the inner box keeps its own size, and the
    // container declares `overflow: visible`, so nothing clips here
    basic_flex_item_shrink => "basic-flex-item-shrink",
    // `:root` matches the `<page>` element
    basic_style_root_selector => "basic-style-root-selector",
    // sibling `<text>` flex items keep their own baselines where nested runs share one
    basic_element_text_baseline => "basic-element-text-baseline",
    // a gradient `color` tiles per element over the union of its own line fragments, and inherits into a nested run
    basic_element_text_linear_gradient_color => "basic-element-text-linear-gradient-color",
    // a nested run inherits weight, and `text-maxline="1"` clamps the wrapped paragraph with no marker: both ruled
    basic_element_text_nest_text => "basic-element-text-nest-text",
    // `align-items` keywords on a block cross axis, where both readings of `start`/`end` agree
    basic_flex_column_align_items => "basic-flex-column-align-items",
    // on `column-reverse`, `start` packs at the block start and `flex-start` at the flow start: the css-align-3 meaning (2026-09-25 ruling)
    basic_flex_column_container_main_axis_justify_content_start_end_with_direction_rtl => "basic-flex-column-container-main-axis-justify-content-start-end-with-direction-rtl",
    // the same contrast on `row`/`row-reverse` under `direction: rtl`, which deliberately differs from web-core's rewritten value
    basic_flex_row_container_main_axis_justify_content_start_end_with_direction_rtl => "basic-flex-row-container-main-axis-justify-content-start-end-with-direction-rtl",
    // `linear-gravity: left|right` is a dropped deprecated longhand, so every box takes the initial flow-start packing
    basic_linear_column_container_main_axis_graverty_right_left_with_direction_rtl => "basic-linear-column-container-main-axis-graverty-right-left-with-direction-rtl",
    // `justify-content: center` is reversal-agnostic, and item order follows the flow
    basic_linear_column_container_main_axis_justify_content_center => "basic-linear-column-container-main-axis-justify-content-center",
    // dropped `linear-gravity`, so each strip packs at its own flow start — which `direction: rtl` now really moves
    basic_linear_row_container_main_axis_graverty_right_left_with_direction_rtl => "basic-linear-row-container-main-axis-graverty-right-left-with-direction-rtl",
    // the same with `start|end` gravity: dropped longhand, flow-start packing in all eight strips
    basic_linear_row_container_main_axis_graverty_start_end_with_direction_rtl => "basic-linear-row-container-main-axis-graverty-start-end-with-direction-rtl",
    // the same with `top|bottom`, a cross-axis keyword on a horizontal main axis; dropped, flow-start packing
    basic_linear_row_container_main_axis_graverty_top_bottom_with_direction_rtl => "basic-linear-row-container-main-axis-graverty-top-bottom-with-direction-rtl",
    // `justify-content: left|right` dropped as unsupported, matching the `linear-gravity` siblings exactly
    basic_linear_row_container_main_axis_justify_content_right_left_with_direction_rtl => "basic-linear-row-container-main-axis-justify-content-right-left-with-direction-rtl",
    // linear keeps Lynx's own aliasing of `start`/`end` to `flex-start`/`flex-end`,
    // so every strip packs at its flow end — the 2026-09-25 ruling that `linear`
    // answers to Lynx and `flex` to CSS.
    basic_linear_row_container_main_axis_justify_content_start_end_with_direction_rtl => "basic-linear-row-container-main-axis-justify-content-start-end-with-direction-rtl",
    // the same aliasing on `column-reverse` columns, where the flow runs upward.
    basic_linear_column_container_main_axis_justify_content_start_end_with_direction_rtl => "basic-linear-column-container-main-axis-justify-content-start-end-with-direction-rtl",
    // two 300px flex items shrink to 50 and nothing clips, which is what both
    // references do with this card's own `defaultOverflowVisible: true`: every
    // view is `visible`, and only the page clips. Upstream's committed golden
    // shows the opposite because it predates the switch and was never redone.
    basic_flex_with_overflow => "basic-flex-with-overflow",
    // an author `position: absolute` on a page does not leave the pager's row: the UA `position: relative !important` holds
    basic_element_x_viewpager_ng_item_position_absolute => "basic-element-x-viewpager-ng-item-position-absolute",
    // `select-index` starts the pager on the second page in the first committed frame, through `--viewpager-initial-index` + `scroll-initial-target`
    basic_element_x_viewpager_ng_select_index => "basic-element-x-viewpager-ng-select-index",
    // an out-of-flow header with no author width fills the coordinator, and the `anchor-size()` slot offset puts the slot at the header's end
    basic_element_x_foldview_ng_header_width => "basic-element-x-foldview-ng-header-width",
    // `globalThis.Object` is truthy in the realm that evaluates the component, so the 100x100 target paints green
    basic_global_this_property_bts => "basic-globalThis-property-bts",
    // the same probe through a BTS `useEffect`: the effect ran, the state update crossed back, and the green re-render reached the first screen
    basic_global_this_property_mts => "basic-globalThis-property-mts",
    // a `position: fixed` child of a scroll-view anchors to the viewport, not the scrollport, and escapes the scroller's clip
    basic_element_scroll_view_fixed => "basic-element-scroll-view-fixed",
    // the scroll-x/scroll-y/enable-scroll axis matrix: the first scroller clips its 1000x1000 image to the scrollport on both axes and the eight empty ones paint nothing (the card's post-tap half is outside a first screen)
    basic_element_scroll_view_scrollable => "basic-element-scroll-view-scrollable",
    // 200 sibling scroll-views mount and lay out in the page's linear column, clipped by the page
    basic_performance_scroll_view_100 => "basic-performance-scroll-view-100",
    // a bare 100x100 scroll-view as the whole app
    basic_scroll_view => "basic-scroll-view",
    // the imperative `animate()` is tap-driven, so the still is the pre-tap state: a 300x300 green view at the page origin, the same geometry and colour as upstream's own `initial`
    api_animate => "api-animate",
    // which animation and transition event names arrive after four taps: untapped, both report lines are empty, which is right, and the `.test-box` boxes lay out where their margins and shrink-wrap put them
    api_animation_event => "api-animation-event",
    // a 0.1s animation with `animation-fill-mode: forwards` holds its 100% keyframe: the box is green once the clock has passed the animation's end
    basic_at_rule_animation => "basic-at-rule-animation",
    // the same with `from`/`to` instead of `0%`/`100%`, inside a `<page>`
    basic_at_rule_animation_from_to => "basic-at-rule-animation-from-to",
    // an `<x-blur-view>` with no `blur-radius` blurs nothing: no presentational hint, no bake, the backdrop sharp through the 1px border
    basic_element_x_blur_view_default => "basic-element-x-blur-view-default",
    // `flex: 1` fills the column and the grandchild keeps its declared 50x50 at the item's content origin.
    basic_flex_1 => "basic-flex-1",
    // Every `align-self` keyword on a column flex cross axis: `auto` takes the container's `align-items: center`, `baseline` synthesizes one at the bottom edge, `stretch` is inert against a definite `width`.
    basic_flex_column_container_items_align_self => "basic-flex-column-container-items-align-self",
    // Linear derives its cross gravity from `align-self` over `align-items`, and `baseline` maps to no gravity, so that item sits at cross-start.
    basic_linear_column_container_items_align_self => "basic-linear-column-container-items-align-self",
    // `linear-direction` is initially `column`, so a bare `display: linear` stacks down the block axis.
    basic_linear_default_orientation => "basic-linear-default-orientation",
    // `linear-weight` is read only by the linear container that owns the item: a grandchild under a flex parent keeps its declared 200x200.
    basic_linear_grand_kid_weight => "basic-linear-grand-kid-weight",
    // A linear item reads no flex longhand, so `flex: 1 2 auto` neither grows nor shrinks it and the second 100px item overflows the 110px row.
    basic_linear_item_do_not_respond_to_flex => "basic-linear-item-do-not-respond-to-flex",
    // `flex-basis: 0` is equally inert in linear: each item is sized from its own `width`, never from a flex base.
    basic_linear_item_do_not_respond_to_flex_basis => "basic-linear-item-do-not-respond-to-flex-basis",
    // Linear items never shrink to fit: two 100px items in a 110px row overflow instead.
    basic_linear_item_do_not_shrink => "basic-linear-item-do-not-shrink",
    // `order` sorts linear items stably, so the two `order: 2` items keep their source order between them.
    basic_linear_item_use_order => "basic-linear-item-use-order",
    // `order` moves a linear item in the paint stack as well as the flow. Upstream skips this case over a z-index bug of its own and has no golden; this is the W3C answer, and deliberately not native's, which paints by insertion order and `z-index` alone.
    basic_linear_item_use_order_affect_z_layout => "basic-linear-item-use-order-affect-z-layout",
    // Adjacent linear items keep both margins, so three 100px items with `margin: 10px` make a 360px container.
    basic_linear_margin_not_collapse => "basic-linear-margin-not-collapse",
    // `direction: rtl` reverses a row linear's main axis, so the first item packs at the right edge.
    basic_linear_orientation_horizontal_with_direction => "basic-linear-orientation-horizontal-with-direction",
    // A positive `linear-weight` replaces the declared main size: `width: 50px` grows to the 100px share of a 200px row at `linear-weight-sum: 2`.
    basic_linear_weight_calced_large_than_size => "basic-linear-weight-calced-large-than-size",
    // The weighted share also shrinks: `width: 200px` is cut to the 100px half-share, so a declared size never wins.
    basic_linear_weight_calced_less_than_size => "basic-linear-weight-calced-less-than-size",
    // `linear-weight-sum` and `linear-weight` divide as floats, so 0.25/0.5 of a 200px row is 100px.
    basic_linear_weight_sum_is_float => "basic-linear-weight-sum-is-float",
    // A class selector from the card's compiled sheet: `.basic` gives `#target` 100x100 and `background-color: pink`.
    basic_class_selector => "basic-class-selector",
    // `color` is non-inheriting by default, so the `<text>` under `style="color:red"` paints black.
    basic_color_not_inherit => "basic-color-not-inherit",
    // `var()` fallback: an undefined custom property falls back to `green` on `background-color`.
    basic_css_var_fallback_background_color => "basic-css-var-fallback-background-color",
    // A fallback nested in a fallback: two undefined custom properties resolve to the innermost `green`.
    basic_css_var_nested_fallback_background_color => "basic-css-var-nested-fallback-background-color",
    // `>` plus `~` with `:not([hidden])`: only the second child beats the `view` type rule, so red sits above green.
    basic_style_combinator => "basic-style-combinator",
    // `rpx` resolves against the 750-unit design width: `20rpx` on a 393px viewport is 10.48px, painted as a snapped 10x10 box.
    basic_rpx_unit => "basic-rpx-unit",
    // A JS-computed `${10+10}rpx` inline style parses as `20rpx`, giving the same box as the literal form.
    basic_rpx_unit_js_value => "basic-rpx-unit-js-value",
    // `vw`/`vh` resolve against the 393x727 viewport: `50vw`/`50vh` are 196.5x363.5, painted as a snapped 197x364 box.
    basic_vw_vh_unit => "basic-vw-vh-unit",
    // Under `enableRemoveCSSScope: false`, an `@import`ed `display: linear` reaches the sub component; `#sub`'s own green 100x100 covers `#index` exactly.
    config_css_remove_scope_false_display_linear => "config-css-remove-scope-false-display-linear",
    // An `@import`ed fragment cascades into the importing component: `common.css`'s `.green` paints `#sub` over the colourless `.basic` parent.
    config_css_remove_scope_false_import_css => "config-css-remove-scope-false-import-css",
    // A mapped child list renders in order after a zero-height sibling: pink, orange, wheat, 100px each.
    basic_list_rendering => "basic-list-rendering",
    // Two `useEffect`-inserted lists keep their place around two static rects: red, green, gap, blue, yellow.
    basic_replaceelement => "basic-replaceelement",
    // A `setState` in a constructor applies but never runs its callback, so the text stays `awesome` (preact#2638).
    basic_setsate_with_cb => "basic-setsate-with-cb",
    // A constructor `setState` is what the first render paints: green, not the initial pink.
    basic_setstate_in_constructor => "basic-setstate-in-constructor",
    // One `useEffect` filling two lists appends them contiguously after the static rect: red, green, blue, yellow.
    basic_useeffect_hydrate => "basic-useeffect-hydrate",
    // The wrapper a mapped list compiles to is transparent to flex: three `flex: 1 1 0` children of a 180px row each get 60px.
    basic_wrapper_element_do_not_impact_layout => "basic-wrapper-element-do-not-impact-layout",
    // A 100x100 inline-styled `<view>` paints its `background: pink` over exactly its border box at the page origin.
    basic_pink_rect => "basic-pink-rect",
    // An `<image>` whose source never loads, with no `placeholder`, paints nothing at all: the box exists, the bitmap does not.
    basic_image => "basic-image",
    // `auto-size` derives a content box that `padding` and `box-sizing` offset like any replaced element's, while the item's auto cross size still stretches to its linear parent.
    basic_element_image_auto_size_with_padding => "basic-element-image-auto-size-with-padding",
    // `border-radius: 50%` clips an `<image>`'s bitmap to a circle inscribed in its border box, not only its background.
    basic_element_image_border_radius => "basic-element-image-border-radius",
    // A CSS-sized rather than content-sized view makes the page the whole viewport, so a `width:100%;height:100%` root `<view>` covers every pixel.
    basic_element_lynx_view_not_auto => "basic-element-lynx-view-not-auto",
    // `border-width` alone paints, because the UA sheet's `border-style: solid` default makes it visible: a 10px black ring around a 180x180 hole.
    basic_element_view_border_style_default => "basic-element-view-border-style-default",
    // Upstream times SSR here, not pixels. 10 block-level 100x100 pink boxes stack gaplessly into one 100-wide column that overflows the viewport.
    basic_performance_div_10 => "basic-performance-div-10",
    // Upstream times SSR here. 100 boxes give the same gapless column, since the viewport only ever shows the first 7.27 of them.
    basic_performance_div_100 => "basic-performance-div-100",
    // Upstream times SSR here. 1000 boxes give the same column; the count never reaches the paint.
    basic_performance_div_1000 => "basic-performance-div-1000",
    // Upstream times SSR here. 10000 boxes give a frame identical to the 10-box card, which is the culling working.
    basic_performance_div_10000 => "basic-performance-div-10000",
    // Upstream times SSR here, and the card's `placeholder.png` exists in no repository, so 200 `<image>` boxes with an unresolvable source and no background correctly paint nothing. The golden pins the absence of a broken-image mark, not the boxes' geometry.
    basic_performance_image_100 => "basic-performance-image-100",
    // Upstream times SSR here. The ~100-rule sheet matches no element — nothing carries a class — so the frame is byte-identical to `basic-performance-div-100`, which is the invariance the card is for.
    basic_performance_large_css => "basic-performance-large-css",
    // Upstream snapshots the SSR HTML here, not pixels. 100 nested unstyled `<div>`s are zero-height block boxes with no background, so an empty frame is correct.
    basic_performance_nest_level_100 => "basic-performance-nest-level-100",
    // Upstream times SSR here. The 10-rule sheet matches nothing, so the frame is byte-identical to both `basic-performance-div-100` and `basic-performance-large-css`.
    basic_performance_small_css => "basic-performance-small-css",
    // Upstream times SSR here. 200 `<text>` boxes stack at an exact 100px pitch, each painting its pink background over the full box and its label on the first line at the box origin.
    basic_performance_text_200 => "basic-performance-text-200",
    // A relative `src` written as a plain string resolves against the card's own URL: the 700x700 logo fills its 100x100 box, unletterboxed and uncropped.
    basic_element_image_src => "basic-element-image-src",
    // `useInitData()` sees the host's page data at first render, so the box is green rather than the no-data pink.
    api_initdata => "api-initdata",
    // A stylesheet the host registers and names in `ViewSources::style_sheets` cascades onto the card's own element — web core's `injectStyleRules` under this engine's name for it.
    api_inject_style_rules => "api-inject-style-rules",
    // `registerDataProcessors`'s default processor runs over the host's raw page data before the first render, turning `mockData` into the green background.
    basic_dataprocessor => "basic-dataprocessor",
    // A tap's `detail.x`/`detail.y` are numbers inside the view: tapping `#tap-area` turns `#target` green. Upstream also offsets the host element by 200px to tell view-relative from page-relative; a view here has no embedding page, so its own viewport is the only frame of reference there is.
    api_bindtap_lynx_view_relative => "api-bindtap-lynx-view-relative",
    // `NativeModules.CustomModule.getColor(data, callback)` reaches the embedder's module, and the answer its callback carries turns the box green.
    api_nativemodules_call => "api-nativemodules-call",
    // `NativeModules.bridge.call('getColor', data, callback)`: the module the embedder registers as `bridge` answers, and the box turns green.
    api_nativemodules_bridge_call => "api-nativemodules-bridge-call",
    // A module call answered 2.5 s late: the box is pink while the callback is held and green once it is invoked.
    api_nativemodules_call_delay => "api-nativemodules-call-delay",
    // `SystemInfo.pixelWidth` and `pixelHeight` are the screen the host names, not the viewport: the two texts read 1234 and 5678.
    api_createlynxview_browserconfig => "api-createLynxView-browserConfig",
    // `lynx.requestAnimationFrame` keeps calling back (`loop` appears) until `lynx.cancelAnimationFrame` stops it (`stop`).
    api_requestanimationframe => "api-requestAnimationFrame",
    // The host's `sendGlobalEvent` reaches a `GlobalEventEmitter` listener with its argument: pink, then green.
    api_sendglobalevent => "api-sendGlobalEvent",
    // The host's `updateData` reaches `useInitData()`: pink, then green.
    api_updatedata => "api-updateData",
    // `updateData` goes through the card's `defaultDataProcessor`, which rewrites the value into the one the render checks for: pink, then green.
    api_updatedata_processdata => "api-updateData-processData",
    // `updateGlobalProps` re-renders the card with the new `lynx.__globalProps`: pink, then blue.
    api_updateglobalprops => "api-updateGlobalProps",
    // The initial `globalProps` are on `lynx.__globalProps` for the first render: pink.
    basic_globalprops => "basic-globalProps",
    // `bindTap` reaches its background handler and the `setState` it makes comes back as a patch: pink, green, pink.
    basic_bindtap => "basic-bindtap",
    // A tap's `detail.x` and `detail.y` are numbers, the only case the handler toggles for: pink, green, pink.
    basic_bindtap_detail => "basic-bindtap-detail",
    // A `bindtap` and a `main-thread:bindtap` on one element both run for one tap: the box turns green, the text reads `BTS Clicked`, and the main-thread handler logs `MTS Clicked`.
    basic_bindtap_simultaneous => "basic-bindtap-simultaneous",
    // `bindwheel` on a view: after a wheel turn over it the text reads `wheel` and the indicator is green.
    basic_bindwheel_view => "basic-bindwheel-view",
    // An `<image>` delivers `bindtap`: the result box goes from red to green.
    basic_element_image_support_tap_event => "basic-element-image-support-tap-event",
    // A wheel turn over a `<list>` scrolls it by 100px and reaches its `bindwheel`: cells 1 to 3, the text `wheel`, a green indicator.
    basic_element_list_bindwheel => "basic-element-list-bindwheel",
    // A keyed re-render replaces list cells 2 and 3 by cell 4 and back: four cells, then three with the red one in the middle, then the first four again.
    basic_element_list_remove_action => "basic-element-list-remove-action",
    // `selectTab({index: 1})` through a selector query, from a tap that bubbles off the pager to the page: the red page, then the green one.
    basic_element_x_viewpager_ng_method_selecttab => "basic-element-x-viewpager-ng-method-selecttab",
    // A bubbled tap's `target` is the child that was hit and carries that child's dataset, a string value and an object value both: the two result boxes turn green.
    basic_event_bubble_dataset => "basic-event-bubble-dataset",
    // `currentTarget.dataset` in the element's own `bindtap`, a string value and an object value both: the two result boxes turn green.
    basic_event_dataset => "basic-event-dataset",
    // A tap's `target.id` and `currentTarget.id` are the element's `id`, the only case the handler toggles for: pink, green, pink.
    basic_event_target_id => "basic-event-target-id",
    // `global-bindTap` on a sibling of the tapped element runs for a tap elsewhere: the observer goes pink, green, pink while the tapped box stays blue.
    basic_global_bind => "basic-global-bind",
    // A `lazy()` component from a container fetched at runtime mounts (green beside blue, no fallback left), and a tap inside it toggles its own state: green, pink, green.
    basic_lazy_component => "basic-lazy-component",
    // The same lazy container, named by a relative `./dist/…` path.
    basic_lazy_component_relative_path => "basic-lazy-component-relative-path",
    // Two instances of one lazy component keep separate state: a tap in the first row turns only its box pink, then the second row's.
    basic_lazy_component_multi => "basic-lazy-component-multi",
    // Two `lazy()` imports of one container, each its own instance: the first row's box turns pink, then both.
    basic_lazy_component_multi_import => "basic-lazy-component-multi-import",
    // A lazy component requested by a tap rather than at boot: the row mounts under the red box, then toggles green, pink, green.
    basic_lazy_component_when_needed => "basic-lazy-component-when-needed",
    // A second instance of an already loaded lazy component, mounted by a tap: it appears under the red box, the two keep separate state, and tapping `#target` again reloads neither.
    basic_lazy_component_when_need_with_itself => "basic-lazy-component-when-need-with-itself",
    // A `useEffect` inside a lazily loaded component runs and its patch paints: the 200x200 box is the effect's pink, not the initial green.
    basic_lazy_component_effect => "basic-lazy-component-effect",
    // `lynx.reload()` called by the card resets component state: green after the first tap, pink again after the tap on `#reload`.
    basic_lynx_reload => "basic-lynx-reload",
    // The host's `reload` resets component state: green after the tap, pink again after the reload.
    basic_reload => "basic-reload",
    // A `main-thread:bindTap` handler runs: the tap logs `hello world` from the main thread, and the box stays pink.
    basic_mts_bindtap => "basic-mts-bindtap",
    // A main-thread handler styles `event.currentTarget` itself, with no background round trip: pink, then green.
    basic_mts_bindtap_change_element_background => "basic-mts-bindtap-change-element-background",
    // A main-thread `touchstart` carries `touches`, `targetTouches`, `changedTouches` and `detail.x`/`detail.y`: one swipe turns all four boxes green.
    basic_mts_bindtouchstart => "basic-mts-bindtouchstart",
    // A `main-thread:ref` is set by the time the tap handler reads it: `setStyleProperties` leaves the box green and 200x200.
    basic_mts_mainthread_ref => "basic-mts-mainthread-ref",
    // `runOnBackground` from a main-thread tap handler reaches a background `setState`: pink, then green.
    basic_mts_run_on_background => "basic-mts-run-on-background",
    // A tap on a child bubbles to a `bindtap` written on the card's own `<page>`: pink, then green.
    basic_page_event => "basic-page-event",
    // An inline style emptied to `{}` loses its last declaration, so the class's pink shows; restoring it paints green again.
    basic_style_remove => "basic-style-remove",
    // One inline property is removed while another stays: the class's pink shows through, then green again.
    basic_style_remove_one_property => "basic-style-remove-one-property",
    // With no `defaultOverflowVisible` in the build the compiler writes `true`, so the unstyled sixth box lets its 50px child overflow like the explicit `overflow: visible` fifth; the two `hidden` boxes clip at the padding edge, and so do the two mixed-axis boxes, whose `visible` axis pairs into a clip (css-overflow-3, as web-core; native clips per axis).
    config_css_default_overflow_visible_unset => "config-css-default-overflow-visible-unset",
    // `SystemInfo.pixelHeight` and `SystemInfo.pixelWidth` are both numbers on the background thread: the effect that checks them turns the 100x100 box from pink to green.
    api_systeminfo_height_width => "api-SystemInfo-height-width",
    // `boundingClientRect` through `lynx.createSelectorQuery()` answers in the view's own coordinates: the blue 100x100 box with `margin: 50px` reports 50/50/150/150, which is what turns `#target` green.
    api_boundingclientrect_lynx_view_relative => "api-boundingclientrect-lynx-view-relative",
    // `lynx.getJSModule('GlobalEventEmitter')`: a `trigger('event1', {color: 'pink'})` 500 ms after mount reaches the `event1` listener alone, so the box goes from orange to pink and never to the `event0` listener's red.
    api_getjsmodule => "api-getJSModule",
    // `lynx.queueMicrotask` runs its callback: the `setColor('green')` queued from the effect turns the 100x100 box from pink to green.
    api_queuemicrotask => "api-queueMicrotask",
    // The card every `api-frame-*` case nests, loaded on its own: three `<text>` lines, where `useInitData().label` and `lynx.__globalProps.message` are absent from the shell's data and render as nothing after `data:` and `global:`.
    api_frame_inner => "api-frame-inner",
    // `auto-size` sizes an `<image>` from its 128x128 bitmap like a replaced element under `max-width/max-height: 100%`: a stretched cross size transfers through the 1:1 ratio (400x400, 200x200, 50x50), a 40px parent caps the width (40x40), and where both axes are fixed by the parent the bitmap fills them unproportionally (40x200).
    basic_element_image_auto_size => "basic-element-image-auto-size",
    // An empty `src` leaves the `placeholder` bitmap on screen: the 128x128 picture fills the 40x40 box at the page origin.
    basic_element_image_placeholder => "basic-element-image-placeholder",
    // `list-type='waterfall'` with `span-count='2'`: each cell goes into the shorter of two 250px lanes at its own 240px width, so the first five cells sit at y = 0, 0, 185, 295 and 357 before the 500px list clips them.
    basic_element_list_waterfall => "basic-element-list-waterfall",
    // A lazy import of a bundle nobody serves (`/dist/nonexistent.web.bundle`) fails without ending the page, and the Suspense fallback stays: `Loading...` is the whole screen.
    basic_lazy_component_fail => "basic-lazy-component-fail",
    // A main-thread ref function handed down as a prop is called from inside the child's own `main-thread:ref` worklet, and the child then styles itself: `#target` has no size until `setStyleProperties` gives it 200x200 green.
    basic_mts_mainthread_nested_ref => "basic-mts-mainthread-nested-ref",
    // `runOnMainThread` from a BTS timer reaches the main-thread ref: the box starts 100x100 pink and the worklet's `setStyleProperties` leaves it 200x200 green.
    basic_mts_run_on_main_thread => "basic-mts-run-on-main-thread",
    // No upstream test opens it. 100 `<div>`s, each with a `bindtap`, give the same gapless 100-wide pink column as `basic-performance-div-100`: a tap listener per box changes nothing in the paint.
    basic_performance_event_div_100 => "basic-performance-event-div-100",
    // `defaultDisplayLinear: false` makes a plain `<view>` a flex row: the two 200px boxes sit side by side and shrink to half the 393px viewport each, where the linear default would stack them.
    config_css_default_display_linear_false => "config-css-default-display-linear-false",
    // With `enableRemoveCSSScope: true` both files' `.basic` are one global sheet and the later `sub.css` wins on both views: green 100x100. `#sub` covers `#index` exactly, so the frame shows the winner on `#sub`; `#index` matches the same two rules.
    config_css_remove_scope_true => "config-css-remove-scope-true",
    // A cell not reached yet is sized by `estimated-main-axis-size-px` and takes its real size once reached: the first scroll to the end stops with 100px of the last cell in view, because the extent counted its 100px estimate, and a second one shows all 200px of it.
    basic_element_list_estimated_main_axis_size_px => "basic-element-list-estimated-main-axis-size-px",
    // The default mode: one item fills the swiper, the dot strip is centred under it, and `current` set to the last item shows the yellow one with the fourth dot lit.
    basic_element_x_swiper_mode_normal => "basic-element-x-swiper-mode-normal",
    // `mode='carousel'`: items are 80% wide and start-aligned, so the next one shows at the right; on the last item the remaining 20% is the swiper's own orange, the end margin that lets it reach the start.
    basic_element_x_swiper_mode_carousel => "basic-element-x-swiper-mode-carousel",
    // `mode='flat-coverflow'`: items are 60% wide and centred with a 20% margin before the first and after the last, so the first screen has orange on the left and the last item orange on the right.
    basic_element_x_swiper_mode_flat_coverflow => "basic-element-x-swiper-mode-flat-coverflow",
    // `current` picks the item a swiper starts on (the left one opens on its second) and turns it when it changes (the right one, tap by tap); a `current` past the last item leaves it on the last.
    basic_element_x_swiper_current => "basic-element-x-swiper-current",
    // `current` changing under a `duration`: green, blue, yellow, and yellow again for a `current` past the end. Every frame is at rest, where `duration` shows nothing; it has no rule here, following web-core (native uses it as the turn's length).
    basic_element_x_swiper_duration => "basic-element-x-swiper-duration",
    // `indicator-color` and `indicator-active-color`: the left strip has the defaults (white, and white at 30%), the right one a violet active dot and three tomato ones.
    basic_element_x_swiper_indicator_color => "basic-element-x-swiper-indicator-color",
    // The dot strip shows with no `indicator-dots` attribute and is gone for a present value that is not `true` (here the literal string `{{false}}`). Showing by default is web-core's; native hides it by default.
    basic_element_x_swiper_indicator_dots => "basic-element-x-swiper-indicator-dots",
    // A `circular` swiper in the default mode, turned through all four items and back to the first by `current`. Nothing in these frames shows the wrap itself: at rest a wrapped turn and a plain one are the same picture.
    basic_element_x_swiper_circular_normal => "basic-element-x-swiper-circular-normal",
    // The same turn through four items and back in `carry` mode, where the current item is at full scale and fills the swiper. As in `circular-normal`, no frame at rest shows the wrap.
    basic_element_x_swiper_circular_carry => "basic-element-x-swiper-circular-carry",
    // A blocking overlay: a tap on the page's blue box shows it — its translucent wrapper tints the page and a red panel sits 300px down with its scroll-views laid out — and a tap on the wrapper above the panel hides it again once the card's 250 ms timer has run.
    basic_element_x_overlay_ng_demo => "basic-element-x-overlay-ng-demo",
    // Four pass-through overlays, one per upstream test, each from a fresh page: a tap on overlay content stays there and a tap beside it reaches the button underneath; an overlay created by a conditional fills the viewport and goes when it is removed; `showoverlay` and `dismissoverlay` are each logged once; a viewport-sized first child takes a tap meant for the page. The first overlay's red panel is centred on its 100x100 container and overflows it on every side; the card's sheet centres it with `justify-content` and `align-items`, rewritten from the deprecated `linear-gravity` pair (see the fixtures README).
    basic_element_x_overlay_ng_playground_test => "basic-element-x-overlay-ng-playground-test",
    // binduiappear on #control (550px tall in a 500px scroll-view) turns it green, and #target, 550px down and clipped away by the scroll-view, gets neither uiappear nor uidisappear and stays green: the first screen is the 300x500 green box, which is both of upstream's toHaveCSS green assertions (exposure.rs registers listener-only elements and clips through the scroll-view)
    api_exposure_no_fake_disappear => "api-exposure-no-fake-disappear",
    // each BlockView row turns #55AA55 from its own binduiappear; rows 1-7 (tops 10..670) are inside the 727px scroll-view and log custom-appear component-1..7 once each in order, rows 8-10 (from 780) are below it and stay orange off screen; the button (534..629, red) and the rows painted past the unclipped 509px .case are the card's own layout under default visible overflow, matching upstream's all-green screenshot of seven green rows
    api_exposure_custom_event_handler => "api-exposure-custom-event-handler",
    // the container's exposure-id follows the tapped box: the boot exposure record (id 1) gives target index: 1; tapping 1 changes nothing and sends nothing; tapping 2, 3, 4 each sends one exposure under the new id and one disexposure under the old id (log: exposure 2/disexposure 1, exposure 3/disexposure 2, exposure 4/disexposure 3, records shaped as web-core's with detail unique-id 0), and frames index-1..index-4 read current/target index/prev index exactly as upstream asserts (1/1/empty, 2/2/1, 3/3/2, 4/4/3)
    api_exposure_change_exposure_id => "api-exposure-change-exposure-id",
    // tapping the 10x10 #button runs lynx.stopExposure() then resumeExposure(); the exposed container (exposure-id show, data-trackstat data) gets one disexposure record whose target.dataset and dataset both carry trackstat, logged as pass:dataset1 and pass:dataset2, the two lines upstream asserts; the card paints nothing, so both frames are blank
    api_exposure_stop_events_has_dataset => "api-exposure-stop-events-has-dataset",
    // tapping #button runs lynx.stopExposure() then resumeExposure(); the disexposure record carries the object-valued dataset trackstat {mydata: data}, logged as pass:dataset2, the line upstream asserts; the card paints nothing, so both frames are blank
    api_exposure_stop_events_has_complex_dataset => "api-exposure-stop-events-has-complex-dataset",
    // the orange .show box (exposure-id show) reads none at boot; stopExposure() sends a disexposure record (frame stopped reads disexposure), stopExposure({sendEvent: false}) sends none and resume finds the box still recorded as exposed so nothing follows (stopped-without-event reads none), stopExposure({sendEvent: true}) sends one again (stopped-with-event reads disexposure): the three counts upstream asserts, in its click order; the 50% log box and the three centred 50px red buttons are the card's layout
    api_exposure_stop_exposure => "api-exposure-stop-exposure",
    // an `<svg content>` 188x492 over its own tiled `background-image` (star.png at 30x30, repeat): the stars tile the whole border box from its top-left and the 100x100 viewBox drawing is scaled to 188x188 and centred vertically (x10..177, y162..329) on top of them, which is the default preserveAspectRatio meet in the box; `#result` has no size, so the card's `bindload` adds nothing to the frame; the markup styles by presentation attributes only
    basic_element_svg_background_image => "basic-element-svg-background-image",
    // an `<svg content>` whose path colour is written `fill=#161823`: the 12x12 viewBox plus sign is drawn in rgb(22,24,35) scaled to the 188px width and centred in the 188x492 box (x16..171, y168..323), so the `#` in the attribute value survives the markup string
    basic_element_svg_hex_color => "basic-element-svg-hex-color",
    // an `<svg content>` whose markup carries non-ASCII characters (`id=✓ à la mode`): the document parses and the eight-path drawing is scaled to 188x188 and centred vertically in the 188x492 box (x10..177, y162..329)
    basic_element_svg_utf8 => "basic-element-svg-utf8",
    // an `<svg content>` sized only by an author class (`.basketball` 500x500): the 100x100 viewBox drawing fills the 500px box (black x26..474, y26..474 in the viewBox's 5..95 range) and the page cuts it at the 393px viewport edge; the CSS is on the host element, not inside the SVG markup
    basic_element_svg_with_css => "basic-element-svg-with-css",
    // an absolutely positioned `<svg content>` (top 8, margin 10, padding 10, 500x500 border box, opacity 0.8) in a 500x500 view: the red rect fills the 480x480 content box from (20,28), at 0.8 opacity (rgb(255,51,51)), cut only by the viewport's right edge; it runs to y507 because the card's `defaultOverflowVisible: true` leaves the outer view `visible` (web-core/css/in_shadow.css:9); upstream's golden stops at y499, the outer view's edge, the same stale-golden case as basic-flex-with-overflow
    basic_element_svg_with_position => "basic-element-svg-with-position",
    // first screen unfolded (toolbar over a 400px header, slot at 400); after the tap on `#tap` `setFoldExpanded({offset: 100, smooth: false})` folds by exactly 100: the header's aqua strip leaves the top, the pink header ends at 299 and the slot's orange/tomato/thistle rows sit at 300..599 under the fixed toolbar, which is upstream's should-be-scrolled-by-method state
    basic_element_x_foldview_ng_method_setfoldexpanded => "basic-element-x-foldview-ng-method-setFoldExpanded",
    // `setFoldExpanded({offset: 99999})` is capped at the fold's range, header 400 minus toolbar 200: after the tap the 400px header is fully under the toolbar (only its pink right edge x220..251 shows beside it) and the slot starts at y200, its orange `#tap` row at 200..299 and salmon to 599, which is the scrollTop of 200 upstream asserts
    basic_element_x_foldview_ng_method_setfoldexpanded_overflow => "basic-element-x-foldview-ng-method-setFoldExpanded-overflow",
    // first screen: items 1..5 (100px, 5px green border) fill the 500px list with the `scrollToPosition` text under it; after the tap `scrollToPosition({position: 10})` puts the item at index 10, item 11, at the list's top with 11..15 visible, as upstream's scroll-to-position screenshot
    basic_element_list_scroll_to_position => "basic-element-list-scroll-to-position",
    // The x-swiper `scrollTo({index, smooth: false})` UI method through a selector query: the left swiper opens on its second (green) item from `current={1}` and the right one on red; each tap on the right swiper invokes `scrollTo` with the next index, so it shows green with the second dot lit, then blue with the third, while the left one stays on green. These are the three states upstream screenshots (scroll-to 1, 2, 3).
    basic_element_x_swiper_method_scroll_to => "basic-element-x-swiper-method-scroll-to",
    // Four pass-through overlays whose hosts carry the card's own `display: flex` class: hidden, none of them shows and no host paints; shown one by one, the four bordered squares stack at the top-left 200px down and nothing of a host tints the page; closing all returns the first screen exactly; and from a fresh page each square, shown, tapped and closed, adds its number, so the count reads 1, 3, 6, 10.
    basic_element_x_overlay_ng_counter_test2 => "basic-element-x-overlay-ng-counter-test2",
    // A `circular` `carousel` swiper turned by `current` through all four items and back to the first: each current item sits at the swiper's start with the next one in the 20% to its right, and on the last item that neighbour is the first item, across the seam; the active dot follows.
    basic_element_x_swiper_circular_carousel => "basic-element-x-swiper-circular-carousel",
    // A view with `exposure-area` 50% below a full-height red view in a 500px scroll-view: scrolled 200px it shows 40% of itself and stays yellow; scrolled 400px it shows 80% and turns green, from one `exposure` record with its id.
    api_exposure_area => "api-exposure-area",
    // Exposure of nested scroll-views: nothing exposed at first, so the colour bar is white; `#x` scrolled to its end brings the orange `lay` text in and the bar turns yellow; back at 0 its `disexposure` turns it white; `#y` scrolled 50px brings 24px of the red `header` in and the bar turns red; back at 0, white again.
    api_exposure_basic => "api-exposure-basic",
    // Element 123 carries `exposure-screen-margin-bottom: 200px`, which grows its own rect upward by 200px (#393), and 789 carries -200px, which shrinks the viewport's bottom by 200px. At scrollTop 200 123 is still 90px below the viewport but its grown rect is inside, so the placeholder is orange; at 800 123 shows its red block and 789, 110px into the viewport, is not exposed and stays unpainted; at 1000 789 is 310px in, past the 200px, and is green. The red block appears with the same update that exposes 123 (#394).
    api_exposure_dynamic_screen_margin => "api-exposure-dynamic-screen-margin",
    // `#index` takes `index.css`'s red `.basic` and `#sub` takes `sub.css`'s green one, each rule guarded by its file's css id (#399). `#sub` covers `#index` exactly, so the frame shows a green 100px square either way; with `sub.css` changed to 50px high the frame showed green over red, which is how the hidden half was checked (2026-10-12).
    config_css_remove_scope_false => "config-css-remove-scope-false",
    // Both files define `.a .b`. The upper box belongs to `index.jsx` and is its green, the lower one belongs to `sub.jsx` and is its orange: the css id guard is on the subject `.b` alone, so the `.a` ancestor from the other file still matches, as in web-core (#399).
    config_css_remove_scope_false_with_descendant_combinator => "config-css-remove-scope-false-with-descendant-combinator",
    // 60% items, the current one flat and centred with 20% on each side, four dots. On the first screen the green neighbour is on the right, after the tap the last item is current with the blue one on the left; the neighbours are turned as `docs/tracking/deviations.md` records for `coverflow` (#395: a parallel projection in 2D, `scale(0.514, 0.8)`, a 14deg skew and a 24.3% translate at the ends of the view timeline): at rest a neighbour stands 12.5% into its timeline, so it is 0.65 as long and 0.86 as high as its box, skewed 10deg with its outer edge lower, and its inner edge stays where layout put it. The numbers are the architect's and these frames pin them.
    basic_element_x_swiper_mode_coverflow => "basic-element-x-swiper-mode-coverflow",
    // Five swipers, one per mode (normal, carousel, carry, coverflow, flat-coverflow), turned from `current` 0 to 1 by a tap. `page-margin` changes nothing, as in web-core; native draws it as a gap between pages. In the fourth, the neighbours are turned as `docs/tracking/deviations.md` records for `coverflow` (#395: a parallel projection in 2D, `scale(0.514, 0.8)`, a 14deg skew and a 24.3% translate at the ends of the view timeline): at rest a neighbour stands 12.5% into its timeline, so it is 0.65 as long and 0.86 as high as its box, skewed 10deg with its outer edge lower, and its inner edge stays where layout put it. The numbers are the architect's and these frames pin them.
    basic_element_x_swiper_page_margin => "basic-element-x-swiper-page-margin",
    // Five `vertical` swipers, one per mode, turned from `current` 0 to 1: the items stack on the block axis, the dots run down the right edge. In the fourth, the column form of coverflow: the neighbours are turned as `docs/tracking/deviations.md` records for `coverflow` (#395: a parallel projection in 2D, `scale(0.514, 0.8)`, a 14deg skew and a 24.3% translate at the ends of the view timeline): at rest a neighbour stands 12.5% into its timeline, so it is 0.65 as long and 0.86 as high as its box, skewed 10deg with its outer edge lower, and its inner edge stays where layout put it. The numbers are the architect's and these frames pin them, with the axes exchanged (`skewX`, the edge nearer the current item fixed).
    basic_element_x_swiper_vertical => "basic-element-x-swiper-vertical",
    // ui margins apply with no switch (#400, the user's word of 2026-10-12, web-core's behaviour), so the card is upstream's text. Element 123's `exposure-ui-margin-top: 50%` is 200px of its own 400px and grows its rect upward; 789's -50% shrinks its rect from the top by 200px. The four frames are the screen-margin card's, pixel for pixel: orange at scrollTop 200, 123 red and 789 unpainted at 800, 789 green at 1000.
    api_exposure_dynamic_ui_margin => "api-exposure-dynamic-ui-margin",
    // Four 60% items, `current` going 0, 1, 2, 3 and back to 0 by taps: the current item is centred with a neighbour in the 20% on each side, the last item to the left of the first and the first to the right of the last, the dots following. Where the scrollport straddles the seam (first screen, `current` 3, and back at 0) one column of the swiper's orange background, one pixel wide and the swiper's height, shows between the last item and the first (x 78 at `current` 0, x 314 at `current` 3): the period is 4 x 235.8 = 943.2px, not a whole number of pixels, and the two copies meet one pixel apart, the approximation `docs/style-assumptions.md` 25 records for the seam copy. The user ruled that column right (2026-10-13).
    basic_element_x_swiper_circular_flat_coverflow => "basic-element-x-swiper-circular-flat-coverflow",
    // The rows turn red on `exposure` and green on `disexposure`. All of `viewpager`, Item1, Text1, Item2 and Text2 are red in the three frames (first screen, after the swipe to tab2, after the swipe back), and none turns green: the page that is not shown starts or ends exactly on the pager's padding edge, what the pager's clip leaves of it is a line on that edge, and an element that only touches an edge is intersecting (`crates/bobcat-core/src/main/exposure.rs`), so the hidden page is exposed from the start and never disexposed. The user ruled that a hidden page touching the edge counts as exposed (2026-10-13). web-core and native both leave it unexposed, and the card's `-10px` screen margins shrink the viewport (393 to 383), not the pager (edge at 353).
    basic_element_x_viewpager_ng_exposure => "basic-element-x-viewpager-ng-exposure",
    // Both `.container` boxes are orange. The lazy bundle's `CSS` section is adopted page-wide and both bundles are compiled unscoped, so the lazy bundle's `.container { background-color: orange }`, adopted later, wins over the card's red on the card's own box too. The user ruled this the expected result (2026-10-13): the isolation between a card's sheet and a lazy bundle's has been removed. Upstream's assertion, taken on the `__QueryComponent` build, is red and orange.
    basic_lazy_component_css => "basic-lazy-component-css",
    // Both `.container` boxes are red. The lazy bundle's own `.container` rule only sizes its box, and the card's `.container { background-color: red }` reaches it, because nothing separates a card's sheet from a lazy bundle's (the user's ruling of 2026-10-13: the isolation has been removed, and this is the expected result). Upstream's assertion, taken on the `__QueryComponent` build, is that the lazy box is not coloured.
    basic_lazy_component_css_blank => "basic-lazy-component-css-blank",
    // Twenty-six `select(...).invoke('boundingClientRect')` probes, each a green row when the answer is what the card's table names and a red one when it is not; the viewport shows the first twenty-one and the label of the next, and the rows are in a `scroll-view` that has nothing to scroll. Every attribute and dataset row in view is green, `[data-test]` included (a dataset entry is also a `data-*` attribute since #404). The four red rows, `s08`, `s12`, `s14` and `s15`, are the ones whose expectation needs a component element: the table expects `#outer-card-1` and `#outer-card-2` to name `<Outer id=...>` and a selector not to cross into it, and a ReactLynx function component is no element, so its `id` names nothing and `.root #root-outer` finds the view inside. web-core has the same four red, for the same reason. `s17_tag` is green: `text` matches the `<text>` element, as the table says; web-core, whose element is `x-text`, has it red.
    api_selectorquery => "api-SelectorQuery",
    // An `<x-blur-view blur-radius='25px'>`, 100px high with a 1px red border, 50px down a view whose background is the Firefox logo. Inside the box the logo is blurred, fading to the white canvas toward the box's left and right ends where the logo ends, with the red label over it; outside it the logo is sharp. The backdrop is baked over the canvas colour (#404), so nothing sharp shows through the blur.
    basic_element_x_blur_view_blur_radius => "basic-element-x-blur-view-blur-radius",
    // Two swipers side by side; taps on the right one turn it to index-1 and index-2 with `indicator-color` changing, a tap on the left one unmounts the right one and the left takes the full width, and another mounts a new right one. Through both width changes the left swiper stays on its second item, green: a snapped container follows the box it was snapped to (css-scroll-snap-1, #404). The new right swiper mounts on index-2, the `current` it is given.
    basic_element_x_swiper_swiper_dynamic => "basic-element-x-swiper-swiper-dynamic",
    // Three gray 100x100 boxes in a column. The page's `.container` and the two lazy bundles' are all page-wide rules, and the last adopted, gray, wins on all three: a card's sheet and a lazy bundle's are not isolated from each other (the user's ruling of 2026-10-13). Upstream's assertion, taken on the `__QueryComponent` build, is red, orange and gray. The card settles now that a frame's wait is the host's own and the view is pumped during it (#403).
    basic_lazy_component_css_multi => "basic-lazy-component-css-multi",
    // With `enableCSSInheritance: true` a text directly under a view, or under a wrapper in it, inherits the view's `color` and `text-decoration` (#404): the three texts under `.typography` are red, underlined, in its italic bold 24px monospace, right-aligned with the blue shadow; the two under `.gradient` take the gradient; `#override-text` keeps its own green 18px with no underline; `#own-gradient` has its own gradient over the inherited typography. After the tap on `#update` the three texts are 30px in `linear-gradient(green, blue)`. A decoration whose decorating box has a gradient `color` is painted black, which the user accepted (2026-10-13; `docs/tracking/deviations.md`, CSS inheritance): `#own-gradient`'s underline on the first screen, and the three texts' after the update.
    config_css_inheritance_true => "config-css-inheritance-true",
    // With the switch off the three texts under `.typography` reset `color` to black and keep the view's typography, the view's own underline is drawn under them in its red, and the two texts under `.gradient` are black: web-core's picture, the recorded decision (`docs/tracking/deviations.md`, CSS inheritance). `#override-text` is green and `#own-gradient` has its gradient either way. After the tap on `#update` the texts are 30px and still black, as upstream asserts, and the view's `color` is a gradient now. A decoration whose decorating box has a gradient `color` is painted black, which the user accepted (2026-10-13; `docs/tracking/deviations.md`, CSS inheritance): the view's underline after the update.
    config_css_inheritance_false => "config-css-inheritance-false",
    // The build leaves `enableCSSInheritance` unset, which is off: the two frames of `config-css-inheritance-false`.
    config_css_inheritance_default => "config-css-inheritance-default",
    // A 300x120 `<frame src='/dist/api-frame-inner.web.bundle'>`: the nested card's main-thread script runs in a ShadowRealm and renders into the frame's shadow root (#409). Its first line reads `frame:ready`, which is what upstream asserts; `data:` and `global:` are empty because this frame passes neither, and a frame has no `data` or `global-props` by the ruling recorded with #409.
    api_frame_src => "api-frame-src",
    // Red at first and still red after 5.6 s with `autoplay` off; a tap on the 10x10 button turns it on, and the swiper turns to green and then blue at its 5 s interval (#408). The second tap writes `autoplay={false}`, and 5.5 s later the swiper has turned once more, to yellow: removing `autoplay` does not stop the timer, the user's ruling of 2026-10-13 and web-core's behaviour. The same four colours as upstream's four web-core screenshots.
    basic_element_x_swiper_autoplay => "basic-element-x-swiper-autoplay",
    // A main-thread tap handler finds the scroll-view with `lynx.querySelector` and invokes `autoScroll({rate: 120, start: true})` (#408). Upstream reads `scrollTop` three seconds in; a screen still scrolling cannot be pinned, so the frame is taken after the run reached the end of the twenty 100px items and stopped there (`autoStop`, its default): items 18 and 19 at the bottom of the 300px scrollport.
    basic_main_query_selector => "basic-main-query-selector",
    // The same `autoScroll({rate: 120, start: true})`, invoked through a `main-thread:ref` (#408). The five gradient items scroll 230px, so the run reaches the end and stops before the frame settles: the last two items in full and the bottom of the third. Upstream asserts `scrollTop` > 100 two seconds in.
    basic_ref_main_invoke_ui_method => "basic-ref-main-invoke-ui-method",
    // Five 100px cells; a 500px wheel turn over the list brings cells 6 to 10. Upstream then asserts that the list's `bindscroll` and `bindscrollend` both ran, and the script waits for the card to log a `scroll` and a `scrollend` event, each with `scrollTop` 500 in its detail (#407).
    basic_element_list_basic => "basic-element-list-basic",
    // The same card with cells sized by their content (110px each): the first screen, the 500px scroll, and the logged `scroll` and `scrollend` events with `scrollTop` 500 (#407).
    basic_element_list_basic_size => "basic-element-list-basic-size",
    // The mouse moves onto the red box, presses and releases there (#407): `mousemove`, `mousedown` and `mouseup` each arrive with `button`, `buttons`, `x`, `y`, `pageX`, `pageY`, `clientX` and `clientY`, and each turns its own 100x100 indicator green.
    basic_bindmouse => "basic-bindmouse",
    // A view with its own `bindKeydown` stays pink when `a` is pressed: a key event goes to what has focus, and nothing on the page does (#407).
    basic_bindkeydown_out_of_view_noop => "basic-bindkeydown-out-of-view-noop",
    // `global-bindKeydown` turns the box green when `a` is pressed (#407).
    basic_global_bindkeydown => "basic-global-bindkeydown",
    // `global-bindKeydown` reads the event's `code`: after `a` is pressed the box shows `KeyA` (#407).
    basic_global_bindkeydown_code => "basic-global-bindkeydown-code",
    // `global-bindKeydown` reads the event's `key`: after Enter is pressed the box shows `Enter` (#407).
    basic_global_bindkeydown_key => "basic-global-bindkeydown-key",
    // `global-bindKeydown` turns the box green only when the event's `shiftKey` is set: `A` pressed with Shift held turns it green (#407).
    basic_global_bindkeydown_shift => "basic-global-bindkeydown-shift",
    // `global-bindKeyup`: with `a` held down the box is still pink, and it turns green when the key is let go (#407).
    basic_global_bindkeyup => "basic-global-bindkeyup",
    // The lazy component mounts as a pink 100x100 box, and a tap on its `main-thread:bindtap` box turns it green through its `main-thread:ref`. The lazy bundle carries the main-thread worklet runtime in its own `main-thread` section since lynx-stack #4066 (canary 02b6e1d2), so its handler is registered.
    basic_lazy_component_mts_bindtap => "basic-lazy-component-mts-bindtap",
    // The page is one sizeless view; the card's subject is that a module-level `globalThis.foo = 123` is the bare identifier `foo` on both threads, and the script waits for the two lines upstream listens for: `btsFoo 123` from the background thread's effect and `mtsFoo 123` from the `runOnMainThread` worklet, in whichever order they arrive.
    api_globalthis => "api-globalThis",
    // A `<text text-maxlength='5'>` that `setState` fills with `123456` one second in; the first screen is taken after two more settles (`LATE`), past that timer, as upstream's is 1.5 s in. It shows `12345`, centred: the value is cut at five characters, with no marker because the text has no `text-overflow` (the 2026-09-14 ruling).
    basic_element_text_maxline_with_setdata => "basic-element-text-maxline-with-setData",
    // One `carry` swiper, its first item at full size. With a finger held 250px left of where it went down, both items are part-scaled about their centres: the red one leaving at about 0.77 and the green one entering at about 0.87, the orange swiper showing between them. web-core's frame at the same moment has them at about 0.65 and 0.97: its keyframes run with the default `ease` and these run linear, a difference `docs/tracking/deviations.md` records for `carry` and `coverflow`. Lifted, the swiper settles on the green item.
    basic_element_x_swiper_mode_carry => "basic-element-x-swiper-mode-carry",
    // The `<svg content=...>` drawing, its 100x100 viewBox scaled to 188x188 and centred in the 188x492 box, and under it `#result`, which the `bindload` handler has turned green. `#result` is 100x100 by the fourth rewrite of the vendored text (2026-10-15, `packages/web-core-e2e-fixtures/README.md`); upstream's own `#result` has no size, so its colour showed nowhere.
    basic_element_svg_bindload => "basic-element-svg-bindload",
    // The fourth rewrite of the vendored text (2026-10-15) gives the boxes sizes and colours: a yellow flex column 300px high with its one child, the red 100x100 linear view, at its start; inside the red view the two blue 20x20 children stack down its vertical main axis from the top. The card also writes the deprecated `linear-gravity: space-between` on that view, which this engine drops by the user's decision of 2026-07 (`docs/tracking/deviations.md`), so the two blue squares are not spread apart; upstream asserts the `justify-content: space-between` web-core maps it to.
    basic_flex_nested_linear_setting => "basic-flex-nested-linear-setting",
    // The effect stores `green` with `lynx.setSharedData` and paints what `lynx.getSharedData` gives back: a green 100x100 box (#424).
    api_setshareddata => "api-setSharedData",
    // No upstream test has this name: it is the second view of upstream's `api-shared-context`. The suite boots `api-setSharedData` first in the same group (`COMPANIONS`), which stores `green`; this view starts orange, and a tap reads `lynx.getSharedData('color')` from the group's shared data and paints it green (#424).
    api_getshareddata => "api-getSharedData",
    // A 300x600 yellow scroll-view of three 300px views. Upstream swipes up 100px from (100, 300) and asserts that a `scroll` event arrived with `scrollTop`, `scrollLeft`, `scrollHeight` and `scrollWidth` in its detail; the script waits for the card to log one (#407). The swipe flings the scroll-view to its end, so the frame shows green over blue at `scrollTop` 300. The card keeps the event's JSON in a `<text style='display:none'>`, hidden since #423. The events' `isDragging` is false even for the ones dispatched while the finger moved, which upstream does not assert.
    basic_element_scroll_view_event_scroll => "basic-element-scroll-view-event-scroll",
    // The same scroll-view; upstream sets `scrollTop` to 200 by script, here the wheel is turned 200px over it, and the card logs one `scrollend` event with `scrollTop` 200 and the other three detail fields upstream asserts (#407). The frame shows the last 100px of red, then green and blue; the `display:none` text that keeps the event is hidden (#423).
    basic_element_scroll_view_event_scrollend => "basic-element-scroll-view-event-scrollend",
    // A `<frame data={{label: 'from-data'}}>`: the nested card reads the frame's `data` as its init data and prints `data:from-data`, what upstream asserts (#425).
    api_frame_data => "api-frame-data",
    // A `<frame global-props={{message: 'from-global-props'}}>`: the nested card's `lynx.__globalProps` is the frame's, and it prints `global:from-global-props`, what upstream asserts (#425).
    api_frame_global_props => "api-frame-global-props",
    // A 300px-wide `<frame auto-height>` with no author height. A frame is `contain: strict`, so without the attribute it would be 0 tall and its paint-contained content would not show; `auto-height` relaxes that to `contain: inline-size layout paint style` (web-core's `contain: content` with its inline-size container, #425), and the frame is as tall as the nested card's three lines, which therefore show. Upstream asserts the attribute and a height above 0.
    api_frame_auto_height => "api-frame-auto-height",
    // A `<frame auto-width>` 120px tall with a yellow background, in a column flex parent that does not stretch it (vendored rewrite, see the fixtures README): the yellow box is 85px wide, the width of the nested card's widest line `frame:ready`, because `auto-width` drops the frame's size containment and the frame takes its content's width (#425).
    api_frame_auto_width => "api-frame-auto-width",
    // A 300x120 `<frame>` whose `data` is `{ label }` from the outer card's state, over an `update` button. First screen: the nested card reads `data:before`. After the tap on `update` the outer card sets `label` to `after` and the nested card re-renders to `data:after`, through the background thread #426 gives a frame's card; `global:` stays empty, the frame passes no `global-props`.
    api_frame_data_update => "api-frame-data-update",
    // An empty 314x40 `x-input`, the 10px blue `#target` and a 200x200 `#result`, pink. The tap on `#target` invokes `seekTo`, a method an `x-input` does not have; the failure carries code 3 and the box turns green (editable text E5, #417).
    api_invoke_fail => "api-invoke-fail",
    // An `x-input` showing `bindblur`, with an empty result line under it. The card's `focus` attribute does not focus the field: web-core's `x-input` observes no `focus` attribute (native Lynx does focus it), so a click outside the field blurs nothing and the result stays empty in both frames, as on web-core. Upstream has no spec for this card.
    basic_element_x_input_bindblur => "basic-element-x-input-bindblur",
    // An `x-input` showing `bindconfirm`. Once the field is clicked and Enter pressed, `bindconfirm` writes the value into the result line, `bindconfirm`, and the caret stands after the value.
    basic_element_x_input_bindconfirm => "basic-element-x-input-bindconfirm",
    // A 300x40 `x-input` showing `bindinput`. After Enter and `fill('foobar')` the field reads `foobar` with the caret after it, and the result line reads `foobar-6-6`, which is what upstream asserts.
    basic_element_x_input_bindinput => "basic-element-x-input-bindinput",
    // An `x-input` showing `bindselection`. With the field focused and `setSelectionRange(2, 5)`, `nds` is highlighted and the result line reads `2-5`, which is what upstream asserts.
    basic_element_x_input_bindselection => "basic-element-x-input-bindselection",
    // A 100x100 red `.blur` box over an `x-input` showing `bindblur`. The tap on the box invokes the field's `blur`; the field never had focus (web-core observes no `focus` attribute, see `basic-element-x-input-bindblur`), so no `blur` fires and the result line stays empty, as on web-core.
    basic_element_x_input_blur => "basic-element-x-input-blur",
    // A 100x100 red `.focus` box over a 300x40 `x-input` showing `bindfocus`. The tap on the box invokes the field's `focus`: the caret appears after the value and `bindfocus` writes it into the result line, which is what upstream asserts.
    basic_element_x_input_focus => "basic-element-x-input-focus",
    // An `x-input` with `input-filter='[^a-zA-Z0-9]'`. After Enter and `fill('foobar!@#)')` the filter removes the four symbols: the field and the result line both read `foobar`, which is what upstream asserts.
    basic_element_x_input_input_filter => "basic-element-x-input-input-filter",
    // `basic-element-x-input-bindinput` on `x-input-ng`: after Enter and `fill('foobar')` the result line reads `foobar-6-6`, which is what upstream asserts.
    basic_element_x_input_ng_bindinput => "basic-element-x-input-ng-bindinput",
    // An empty 300x40 `x-input` whose placeholder `foo` is red, bold and 20px from its `placeholder-color`, `placeholder-font-weight` and `placeholder-font-size` attributes.
    basic_element_x_input_placeholder => "basic-element-x-input-placeholder",
    // A red `.click` box, a blue `.clear` box, an `x-input` bound to the state `value` and a result line of the same state, all empty at first. A tap on the red box sets the state to `1`: the field and the line both read `1`. A tap on the blue box invokes the field's `setValue('')`: the field empties and the line, which reads the state, keeps `1`. Upstream has no spec for this card.
    basic_element_x_input_setvalue => "basic-element-x-input-setValue",
    // A red `.click` box over an empty `x-input` bound to the state `value`. A tap on the box sets it to `1`: the field and the result line both read `1`. Upstream has no spec for this card.
    basic_element_x_input_value => "basic-element-x-input-value",
    // A full-width 30px `x-textarea`, empty. Upstream's three events, each awaited in the log: a click focuses it (`focus`, value empty; the caret at its start), a click outside blurs it (`blur`, value empty), and `fill('value')` gives `input` with value `value` and selection 5 to 5; the field then reads `value` with the caret after it.
    basic_element_x_textarea_bindinput => "basic-element-x-textarea-bindinput",
    // An `x-textarea` showing `bindselection`. With the field focused and `setSelectionRange(2, 5)`, `nds` is highlighted and the result line reads `2-5`, which is what upstream asserts.
    basic_element_x_textarea_bindselection => "basic-element-x-textarea-bindselection",
    // A full-width 100px `x-textarea` with no border whose value `111` is red from the host's `color`.
    basic_element_x_textarea_color => "basic-element-x-textarea-color",
    // An `x-textarea` with `input-filter='[^a-zA-Z0-9]'`. Enter inserts a line break, then `fill('foobar!@#)')` replaces the value and the filter removes the four symbols: the field and the result line both read `foobar`, which is what upstream asserts.
    basic_element_x_textarea_input_filter => "basic-element-x-textarea-input-filter",
    // Six 5px sky-blue blocks over eleven 30px `x-textarea`s. First screen: `maxlength={1}` with value `OO` shows `O`; the six dynamic fields show nothing but the one whose value is `OO` from the start; the four fixed ones with `maxlength` 2 to 5 show `OO`. After the six blocks are clicked, every dynamic field reads `OO`: a value of two units under a limit lowered to 2 is kept whole, whichever is written first.
    basic_element_x_textarea_maxlength => "basic-element-x-textarea-maxlength",
    // A 50px sky-blue block over a 300px `x-textarea` with `maxlines={3}` showing the four lines `ccc`, `aa`, `bbb`, `a`; after the block is clicked, `a`, `aa`, `bbb`, `ccc`. Four lines over a limit of 3 are right in both frames: the limit cuts only what an edit inserts and never a script's write (the native rule, ruled 2026-10-19), and web-core has no `maxlines` at all.
    basic_element_x_textarea_maxlines => "basic-element-x-textarea-maxlines",
    // A 200px sky-blue block over two empty 30px `x-textarea`s: the first shows its grey placeholder `placeholder`, the second none. After the block is clicked the second shows `I am placeholder`.
    basic_element_x_textarea_placeholder => "basic-element-x-textarea-placeholder",
    // Five empty 30px `x-textarea`s, each placeholder `placeholder` in #DFA878: 20px from `placeholder-font-size`, 14px by default, 30px from the host's `font-size: 30px`, 20px again where the attribute outranks that `font-size`, and 14px.
    basic_element_x_textarea_placeholder_font_size => "basic-element-x-textarea-placeholder-font-size",
    // A 200px sky-blue block over two empty 30px `x-textarea`s. The first placeholder is #DFA878, 20px, normal weight in both frames; the second is #D71313, 12px, bold, and after the block is clicked #F0DE36, 16px, normal.
    basic_element_x_textarea_placeholder_style => "basic-element-x-textarea-placeholder-style",
}
