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
    // With `enableCSSSelector: false` the sheet still cascades by real specificity: `.parent .background` (pink) beats both single-class rules.
    config_css_selector_false_multi_level_selector => "config-css-selector-false-multi-level-selector",
    // A bare `view` type selector still matches under `enableCSSSelector: false`, giving `#target` yellow at 100x100.
    config_css_selector_false_type_selector => "config-css-selector-false-type-selector",
    // First screen only, since the suite drives no reload: `.parent` gives `#target` a 200x200 red box under `enableCSSSelector: false`.
    config_css_selector_false_reload => "config-css-selector-false-reload",
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
    // An inline colour and a class added in one update, then both removed: inline wins, so yellow, red, yellow. Nothing in this card depends on `enableCSSSelector`.
    config_css_selector_false_inline_css_change_same_time => "config-css-selector-false-inline-css-change-same-time",
    // The inline colour changes in the same update that adds or removes a class: inline wins each time, so green, yellow, green. Nothing in this card depends on `enableCSSSelector`.
    config_css_selector_false_remove_css_and_style_collapsed => "config-css-selector-false-remove-css-and-style-collapsed",
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
}
