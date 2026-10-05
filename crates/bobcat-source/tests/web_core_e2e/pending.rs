// Cards nobody has read against their own source yet, or whose rendering is
// wrong in a way the reason names. No golden belongs to any of them; see
// `web_core_e2e.rs`.
//
// Moving one into `verified.rs` is the whole job this suite exists for: read
// what the case tests, decide whether this engine answers it correctly, and
// only then accept the picture.

pending! {
    "api-SelectorQuery" => "not read yet",
    "api-SystemInfo" => "not read yet",
    "api-SystemInfo-height-width" => "not read yet",
    "api-bindlauoutchange" => "not read yet",
    "api-bindlayoutchange-lynx-view-relative" => "not read yet",
    "api-bindtap-lynx-view-relative" => "not read yet",
    "api-boundingclientrect-lynx-view-relative" => "not read yet",
    "api-createLynxView-browserConfig" => "not read yet",
    "api-devtool-event" => "not read yet",
    "api-dispose" => "not read yet",
    "api-error" =>
        "the card throws on both threads, so a blank screen is the only painting there can be; what upstream asserts is the error report's sourcemap offset and stack, which no capture can settle. web-core also hides the host element, where this engine deliberately keeps the view (#330)",
    "api-error-bts" =>
        "a BTS-only throw, same shape: upstream asserts `fileName` (`app-service.js`), where this engine names the realm instead",
    "api-error-mts" =>
        "an MTS-only throw: blank is consistent and the view surviving to `ScriptFinished` is #330 working; upstream asserts the report's `fileName` (`lepus.js`), which this engine does not carry",
    "api-exposure-area" => "not read yet",
    "api-exposure-basic" => "not read yet",
    "api-exposure-change-exposure-id" => "not read yet",
    "api-exposure-custom-event-handler" => "not read yet",
    "api-exposure-dynamic-screen-margin" => "not read yet",
    "api-exposure-dynamic-ui-margin" => "not read yet",
    "api-exposure-no-fake-disappear" => "not read yet",
    "api-exposure-stop-events-has-complex-dataset" => "not read yet",
    "api-exposure-stop-events-has-dataset" => "not read yet",
    "api-exposure-stop-exposure" => "not read yet",
    "api-frame-auto-height" => "not read yet",
    "api-frame-auto-width" => "not read yet",
    "api-frame-bindload" => "not read yet",
    "api-frame-data" => "not read yet",
    "api-frame-data-update" => "not read yet",
    "api-frame-element-map" => "not read yet",
    "api-frame-global-props" => "not read yet",
    "api-frame-inner" => "not read yet",
    "api-frame-src" => "not read yet",
    "api-get-path-info" => "not read yet",
    "api-getJSModule" => "not read yet",
    "api-getSharedData" => "not read yet",
    "api-global-disallowed-vars" =>
        "DEFECT: `MTS_CHUNK_PREAMBLE` (crates/bobcat-core/src/esm.rs) declares none of the disallowed globals the BTS preamble shadows, so MTS code naming `navigator`/`postMessage` raises a ReferenceError where web-core's lepus wrapper gives `undefined`. The card paints only a sizeless view, so the blank screen hides it",
    "api-globalThis" =>
        "a module-level `globalThis.foo` must be a bare identifier in both realms and inside a `runOnMainThread` worklet; the card paints only a sizeless view, so a capture says nothing about it",
    "api-initdata" => "not read yet",
    "api-inject-style-rules" => "not read yet",
    "api-invoke-fail" => "not read yet",
    "api-invoke-success" => "not read yet",
    "api-lynx-performance" => "not read yet",
    "api-nativemodules-bridge-call" => "not read yet",
    "api-nativemodules-call" => "not read yet",
    "api-nativemodules-call-delay" => "not read yet",
    "api-queueMicrotask" => "not read yet",
    "api-report-error" => "not read yet",
    "api-requestAnimationFrame" => "not read yet",
    "api-sendGlobalEvent" => "not read yet",
    "api-set-release" =>
        "`_SetSourceMapRelease` is a deliberate no-op here and `ScriptReported` carries no release, so the report upstream asserts cannot match; the blank screen is consistent but settles nothing",
    "api-set-release-bts" =>
        "DEFECT: the BTS `app` behind `lynxCoreInject.tt` has no `setSourceMapRelease`, so the card hits a TypeError before its own error",
    "api-setSharedData" => "not read yet",
    "api-updateData" => "not read yet",
    "api-updateData-callback" => "not read yet",
    "api-updateData-processData" => "not read yet",
    "api-updateGlobalProps" => "not read yet",
    "basic-bindkeydown-out-of-view-noop" => "not read yet",
    "basic-bindmouse" => "not read yet",
    "basic-bindtap" => "not read yet",
    "basic-bindtap-detail" => "not read yet",
    "basic-bindtap-simultaneous" => "not read yet",
    "basic-bindwheel-view" => "not read yet",
    "basic-class-selector" => "not read yet",
    "basic-color-not-inherit" => "not read yet",
    "basic-css-var-fallback-background-color" => "not read yet",
    "basic-css-var-nested-fallback-background-color" => "not read yet",
    "basic-dataprocessor" => "not read yet",
    "basic-element-image-auto-size" => "not read yet",
    "basic-element-image-auto-size-with-padding" => "not read yet",
    "basic-element-image-border-radius" => "not read yet",
    "basic-element-image-placeholder" => "not read yet",
    "basic-element-image-src" => "not read yet",
    "basic-element-image-support-tap-event" => "not read yet",
    "basic-element-input-bindinput" => "not read yet",
    "basic-element-list-basic" => "not read yet",
    "basic-element-list-basic-size" => "not read yet",
    "basic-element-list-bindwheel" => "not read yet",
    "basic-element-list-estimated-main-axis-size-px" => "not read yet",
    "basic-element-list-estimated-main-axis-size-px-waterfall" => "not read yet",
    "basic-element-list-horizontal-estimated-main-axis-size-px" => "not read yet",
    "basic-element-list-horizontal-estimated-main-axis-size-px-waterfall" => "not read yet",
    "basic-element-list-remove-action" => "not read yet",
    "basic-element-list-scroll-to-position" =>
        "the first screen is right (five 100px cells in a 500px list); `scrollToPosition` is a UI method that does not exist and the card drives it from a tap",
    "basic-element-list-waterfall" => "not read yet",
    "basic-element-lynx-view-not-auto" => "not read yet",
    "basic-element-scroll-view-event-scroll" =>
        "the pre-swipe frame is right; `scroll-view` emits no `scroll` event and a swipe cannot be settled by a capture",
    "basic-element-scroll-view-event-scrollend" =>
        "same card with `bindscrollend`: the event does not exist and the gesture cannot be settled",
    "basic-element-scroll-view-event-scrolltolower" =>
        "the bottom-threshold event: the threshold observer behind `scrolltolower` is not built, and the frame cannot swipe",
    "basic-element-scroll-view-event-scrolltoupper" =>
        "the upper-threshold event, same reason",
    "basic-element-scroll-view-scroll-to-index" =>
        "`initial-scroll-to-index={1}` must land the scrollport on the second child at boot; the attribute does not exist here, so we start at the first. Statically wrong, and a missing attribute rather than an interaction",
    "basic-element-svg-background-image" => "not read yet",
    "basic-element-svg-bindload" => "not read yet",
    "basic-element-svg-hex-color" => "not read yet",
    "basic-element-svg-utf8" => "not read yet",
    "basic-element-svg-with-css" => "not read yet",
    "basic-element-svg-with-position" => "not read yet",
    "basic-element-text-bindlayout" =>
        "the layout event is produced but never dispatched, so the result row stays empty",
    "basic-element-text-bindselectionchange" =>
        "selection events are not dispatched; the paragraph itself is right",
    "basic-element-text-maxline-with-setData" =>
        "the setState carrying the clamped string lands at 1000 ms, past this suite's 500 ms settle",
    "basic-element-text-set-native-props-text" =>
        "the setNativeProps push is not retargeted onto the leading raw-text (recorded GAP)",
    "basic-element-text-set-native-props-text-do-not-change-inline-text" =>
        "same un-retargeted push (recorded GAP)",
    "basic-element-text-set-native-props-with-maxlength" =>
        "same un-retargeted push; the clamp itself is right but the paragraph box is two lines tall for one painted line",
    "basic-element-text-set-native-props-with-setData" =>
        "same un-retargeted push; the pre-tap frame is right",
    "basic-element-text-text-selection" =>
        "what a selection drag highlights is an interaction no first screen carries; selection is out of scope",
    "basic-element-view-border-style-default" => "not read yet",
    "basic-element-x-audio-tt-play" => "not read yet",
    "basic-element-x-blur-view-blur-radius" =>
        "DEFECT: the backdrop bake clears to transparent black and never carries the canvas base colour, and the filtered texture is drawn over the still-painted unfiltered backdrop, so wherever the baked alpha is below one the sharp backdrop shows through — a visible logo silhouette inside the blurred box. The blur itself is spec-exact (sigma 25 matches Chromium to 1-2 levels over opaque areas). Not one of the recorded backdrop-filter narrowings",
    "basic-element-x-foldview-ng-method-setFoldExpanded" =>
        "the unfolded first screen matches run for run; `setFoldExpanded` is deliberately not implemented and the card is a tap + method",
    "basic-element-x-foldview-ng-method-setFoldExpanded-overflow" =>
        "same unfolded screen; the method is not implemented and a capped fold offset is invisible while unfolded",
    "basic-element-x-input-bindblur" =>
        "`<x-input>` is implemented nowhere, so the box is geometrically right but its `value` never paints; the blur event has nothing to fire from",
    "basic-element-x-input-bindconfirm" => "not read yet",
    "basic-element-x-input-bindfocus" => "not read yet",
    "basic-element-x-input-bindinput" => "not read yet",
    "basic-element-x-input-bindselection" => "not read yet",
    "basic-element-x-input-blur" =>
        "the same missing `<x-input>`: the red square and the input's box are placed right, the value is absent, and there is no focus machinery for the tap to drive",
    "basic-element-x-input-focus" => "not read yet",
    "basic-element-x-input-getValue" => "not read yet",
    "basic-element-x-input-input-filter" =>
        "the same missing `<x-input>`: a geometrically right, empty box where `input-filter` would strip punctuation as the user types",
    "basic-element-x-input-ng-bindinput" => "not read yet",
    "basic-element-x-input-placeholder" => "not read yet",
    "basic-element-x-input-placeholder-pseudo-element" => "not read yet",
    "basic-element-x-input-setValue" => "not read yet",
    "basic-element-x-input-type" => "not read yet",
    "basic-element-x-input-value" => "not read yet",
    "basic-element-x-overlay-ng-counter-test2" =>
        "there is no `x-overlay-ng`, so four overlays that are hidden on the first screen paint in place and cover the page",
    "basic-element-x-overlay-ng-demo" =>
        "there is no `x-overlay-ng`: a hidden overlay's scrim paints over the whole page",
    "basic-element-x-overlay-ng-playground-test" =>
        "there is no `x-overlay-ng`: a hidden modal leaks into the bottom half of the page",
    "basic-element-x-refresh-view-demo" => "not read yet",
    "basic-element-x-swiper-autoplay" =>
        "autoplay paging on an interval; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-bindchange" =>
        "the `change` payload across manual, autoplay and programmatic paging; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-bindscrollend" =>
        "the `scrollend` payload; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-bindscrollstart" =>
        "the `scrollstart` payload; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-circular-carousel" =>
        "`circular` wrap in carousel mode; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-circular-carry" =>
        "`circular` wrap in carry mode; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-circular-coverflow" =>
        "`circular` wrap in coverflow mode; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-circular-flat-coverflow" =>
        "`circular` wrap in flat-coverflow mode; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-circular-normal" =>
        "`circular` wrap in the default mode; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-current" =>
        "`current={1}` must start on the second page with no interaction at all, and we stack all four pages: `x-swiper` is not a component here. Statically wrong, not an interaction gap",
    "basic-element-x-swiper-duration" =>
        "the paging animation duration across taps; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-indicator-color" =>
        "`indicator-color`/`indicator-active-color` must tint the dot strip, and no indicator is drawn at all — shadow-part machinery no UA sheet can carry. Statically wrong",
    "basic-element-x-swiper-indicator-dots" =>
        "dots on against dots off: both swipers render identically here because neither draws dots. Statically wrong",
    "basic-element-x-swiper-interval" =>
        "autoplay intervals of 5000, 1000 and 0; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-method-scroll-to" =>
        "`invoke({method: 'scrollTo'})` from a tap; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-mode-carousel" =>
        "carousel snap offsets, side pages peeking; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-mode-carry" =>
        "carry mode with the dots off; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-mode-coverflow" =>
        "coverflow's skewed peek; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-mode-flat-coverflow" =>
        "the flat variant of coverflow; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-mode-normal" =>
        "default paging: one page plus a dot strip; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-page-margin" =>
        "`page-margin` gaps between pages in all five modes; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-swiper-dynamic" =>
        "remounting a swiper on tap and re-reading `indicator-color`; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-swiper-vertical" =>
        "vertical paging across five modes; `x-swiper` is not a component here, so its items get no border box, no clip and no indicator, and the first screen stacks them instead of showing one page",
    "basic-element-x-textarea-bindinput" => "not read yet",
    "basic-element-x-textarea-bindselection" => "not read yet",
    "basic-element-x-textarea-color" => "not read yet",
    "basic-element-x-textarea-disabled" => "not read yet",
    "basic-element-x-textarea-focus" => "not read yet",
    "basic-element-x-textarea-getValue" => "not read yet",
    "basic-element-x-textarea-input-filter" =>
        "`<x-textarea>` is equally unimplemented: an empty 300x42 box where the initial value belongs",
    "basic-element-x-textarea-maxlength" => "not read yet",
    "basic-element-x-textarea-maxlines" => "not read yet",
    "basic-element-x-textarea-min-height-max-height" => "not read yet",
    "basic-element-x-textarea-placeholder" => "not read yet",
    "basic-element-x-textarea-placeholder-font-size" => "not read yet",
    "basic-element-x-textarea-placeholder-style" => "not read yet",
    "basic-element-x-viewpager-ng-allow-horizontal-gesture" =>
        "the first screen is right; `allow-horizontal-gesture={false}` only shows under the swipe it refuses",
    "basic-element-x-viewpager-ng-bindchange" =>
        "the first screen is right; the pager's `change` event is deliberately not implemented and the card is swipe-driven",
    "basic-element-x-viewpager-ng-bindchange-select-tab" =>
        "the first screen is right; tap + `selectTab` + `change` are all outside a first screen, and `change` is not implemented",
    "basic-element-x-viewpager-ng-bindoffsetchange" =>
        "the first screen is right; `offsetchange` is deliberately not implemented and the card is swipe-driven",
    "basic-element-x-viewpager-ng-exposure" =>
        "geometry is right, but exposure tracking does not exist, so the five texts never repaint as pages come into view",
    "basic-element-x-viewpager-ng-method-selecttab" =>
        "the pre-tap screen is right; a `selectTab` call on tap cannot be settled by a first screen",
    "basic-element-x-webview-bindmessage" => "not read yet",
    "basic-event-bubble-dataset" => "not read yet",
    "basic-event-dataset" => "not read yet",
    "basic-event-target-id" => "not read yet",
    "basic-event-trigger" => "not read yet",
    "basic-flex-1" => "not read yet",
    "basic-flex-column-container-items-align-self" => "not read yet",
    "basic-flex-nested-linear-setting" =>
        "the case asserts computed style only and neither box has a background, so a first screen shows nothing either way",
    "basic-global-bind" => "not read yet",
    "basic-global-bindkeydown" => "not read yet",
    "basic-global-bindkeydown-code" => "not read yet",
    "basic-global-bindkeydown-key" => "not read yet",
    "basic-global-bindkeydown-shift" => "not read yet",
    "basic-global-bindkeyup" => "not read yet",
    "basic-globalProps" => "not read yet",
    "basic-image" => "not read yet",
    "basic-lazy-component" => "not read yet",
    "basic-lazy-component-css" => "not read yet",
    "basic-lazy-component-css-blank" => "not read yet",
    "basic-lazy-component-css-multi" => "not read yet",
    "basic-lazy-component-css-selector-false-exchange-class" => "not read yet",
    "basic-lazy-component-css-selector-false-inline-css-change-same-time" => "not read yet",
    "basic-lazy-component-css-selector-false-inline-remove-css-remove-inline" => "not read yet",
    "basic-lazy-component-css-selector-false-multi-level-selector" => "not read yet",
    "basic-lazy-component-css-selector-false-remove-all" => "not read yet",
    "basic-lazy-component-css-selector-false-remove-css-and-reuse-css" => "not read yet",
    "basic-lazy-component-css-selector-false-remove-css-and-style-collapsed" => "not read yet",
    "basic-lazy-component-css-selector-false-remove-inline-style-and-reuse-css" => "not read yet",
    "basic-lazy-component-css-selector-false-type-selector" => "not read yet",
    "basic-lazy-component-effect" => "not read yet",
    "basic-lazy-component-fail" => "not read yet",
    "basic-lazy-component-mts-bindtap" => "not read yet",
    "basic-lazy-component-multi" => "not read yet",
    "basic-lazy-component-multi-import" => "not read yet",
    "basic-lazy-component-relative-path" => "not read yet",
    "basic-lazy-component-when-need-with-itself" => "not read yet",
    "basic-lazy-component-when-needed" => "not read yet",
    "basic-linear-column-container-items-align-self" => "not read yet",
    "basic-linear-default-orientation" => "not read yet",
    "basic-linear-grand-kid-weight" => "not read yet",
    "basic-linear-item-do-not-respond-to-flex" => "not read yet",
    "basic-linear-item-do-not-respond-to-flex-basis" => "not read yet",
    "basic-linear-item-do-not-shrink" => "not read yet",
    "basic-linear-item-use-order" => "not read yet",
    "basic-linear-item-use-order-affect-z-layout" => "not read yet",
    "basic-linear-margin-not-collapse" => "not read yet",
    "basic-linear-orientation-horizontal-with-direction" => "not read yet",
    "basic-linear-weight-calced-large-than-size" => "not read yet",
    "basic-linear-weight-calced-less-than-size" => "not read yet",
    "basic-linear-weight-sum-is-float" => "not read yet",
    "basic-list-rendering" => "not read yet",
    "basic-lynx-reload" => "not read yet",
    "basic-main-query-selector" => "not read yet",
    "basic-mts-bindtap" => "not read yet",
    "basic-mts-bindtap-change-element-background" => "not read yet",
    "basic-mts-bindtouchstart" => "not read yet",
    "basic-mts-mainthread-nested-ref" => "not read yet",
    "basic-mts-mainthread-ref" => "not read yet",
    "basic-mts-run-on-background" => "not read yet",
    "basic-mts-run-on-main-thread" => "not read yet",
    "basic-mts-systeminfo" => "not read yet",
    "basic-page-event" => "not read yet",
    "basic-performance-div-10" => "not read yet",
    "basic-performance-div-100" => "not read yet",
    "basic-performance-div-1000" => "not read yet",
    "basic-performance-div-10000" => "not read yet",
    "basic-performance-event-div-100" => "not read yet",
    "basic-performance-image-100" => "not read yet",
    "basic-performance-large-css" => "not read yet",
    "basic-performance-nest-level-100" => "not read yet",
    "basic-performance-small-css" => "not read yet",
    "basic-performance-text-200" => "not read yet",
    "basic-pink-rect" => "not read yet",
    "basic-ppx-unit" => "not read yet",
    "basic-ref-main-invoke-ui-method" => "not read yet",
    "basic-reload" => "not read yet",
    "basic-replaceelement" => "not read yet",
    "basic-rpx-unit" => "not read yet",
    "basic-rpx-unit-js-value" => "not read yet",
    "basic-setsate-with-cb" => "not read yet",
    "basic-setstate-in-constructor" => "not read yet",
    "basic-style-combinator" => "not read yet",
    "basic-style-remove" =>
        "the removal happens on tap, so a first screen cannot show what the case is named for",
    "basic-style-remove-one-property" => "not read yet",
    "basic-useeffect-hydrate" => "not read yet",
    "basic-vw-vh-unit" => "not read yet",
    "basic-wrapper-element-do-not-impact-layout" => "not read yet",
    "config-css-default-display-linear-false" => "not read yet",
    "config-css-default-overflow-visible-unset" => "not read yet",
    "config-css-inheritance-default" => "not read yet",
    "config-css-inheritance-false" => "not read yet",
    "config-css-inheritance-true" => "not read yet",
    "config-css-remove-scope-false" => "not read yet",
    "config-css-remove-scope-false-display-linear" => "not read yet",
    "config-css-remove-scope-false-import-css" => "not read yet",
    "config-css-remove-scope-false-with-descendant-combinator" => "not read yet",
    "config-css-remove-scope-true" => "not read yet",
    "config-css-selector-false-exchange-class" => "not read yet",
    "config-css-selector-false-inline-css-change-same-time" => "not read yet",
    "config-css-selector-false-inline-remove-css-remove-inline" => "not read yet",
    "config-css-selector-false-multi-level-selector" => "not read yet",
    "config-css-selector-false-reload" => "not read yet",
    "config-css-selector-false-remove-all" => "not read yet",
    "config-css-selector-false-remove-css-and-reuse-css" => "not read yet",
    "config-css-selector-false-remove-css-and-style-collapsed" => "not read yet",
    "config-css-selector-false-remove-inline-style-and-reuse-css" => "not read yet",
    "config-css-selector-false-type-selector" => "not read yet",
    "config-mixed-01" => "not read yet",
}
