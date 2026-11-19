# Still ahead

Linked from the Workspace map of the root [AGENTS.md](../AGENTS.md).

What the runtime layer does not implement yet, each recorded with the code that
would host it:

- **The entry-name CSS guard.** css-id scoping is implemented, but a lazy
  bundle's own `StyleInfo` is not mounted, so web-core's `l-e-name` /
  `:not([l-e-name])` guard that would separate it from the host card's rules
  is not synthesized and `__SetCSSId` ignores `entryName`; both land with
  lazy-bundle stylesheet mounting (`docs/tracking/deviations.md`).
- **The list surface.** `crates/bobcat-core/src/main/tree/list.rs` carries
  what a UA sheet can say about `list` and `list-item`: the scroll axis, the
  `list-type` layout modes (`flow` = grid, `waterfall` = `grid-lanes`), cells
  virtualized by `content-visibility: auto` with a `100cqh` estimate fallback,
  and the `span-count`/`column-count`/`sticky-offset`/
  `estimated-main-axis-size-px` `attr()` declarations. The span registration
  defaults to one; `span-count` takes precedence over `column-count`, and the
  track count is at least one. Sticky offsets are numeric pixels clamped at
  zero. Cell estimates feed `contain-intrinsic-size` directly through
  `attr(estimated-main-axis-size-px px, 100cqh)` (`100cqw` horizontally).
  Estimates require nonnegative numbers; no Rust estimate adapter is needed.
  Remembered sizes still win through the normal containment path.
  Underneath it, the data protocol —
  `__SetAttribute(element, "update-list-info", …)` — delivers a compiled
  `<list>`'s cells as real element children, which is the only path one
  receives children on. `<list>` is a `terminus::link::CustomElement` for its UI
  methods only (`scrollToPosition`, `scrollBy`, `autoScroll`,
  `getScrollContainerInfo`, `getVisibleCells`): there is no cell recycling,
  no threshold or scroll events, and no sticky-bottom or snap rules
  (`docs/tracking/deviations.md`).
- **Gesture detectors and the arena.** `crates/terminus/src/painter/gesture.rs`
  has no fling or velocity, no `:active` driving, no `consume-slide-event`, no
  per-element `GestureDetector`/arena relations and no `pointerover`/`out`/
  `enter`/`leave`, mouse boundary events, `auxclick` or `contextmenu`
  (`click`, `dblclick` and the compatibility mouse events are built); `tapSlop` is
  the default 50 px rather than the page config's.
- **The rest of the `<image>` element surface.** `src`, `placeholder`,
  `mode`, `auto-size` and `blur-radius` work, `load` (with the bitmap's
  natural size) and `error` fire, non-bubbling, through the component-event
  queue, and an animated `src` plays under `loop-count` and `autoplay` with
  its four methods and three events; `cap-insets` does not.
- **The rest of the `<video>` element surface.** `src`, `loop`, `muted` and
  `object-fit`, the `play` and `pause` UI methods and the `firstframe`,
  `ended` and `error` events are built (`docs/picture-source-design.md`,
  "Video"). Not built: the `volume`, `speed`, `mode` and
  `timeupdate-interval` attributes (ignored), the `stop` and `seek` UI
  methods (code 3), and the `playing`, `paused`, `stopped`, `timeupdate`,
  `looped` and `buffering` events (never fired). `play` answers at once
  rather than when playback starts. Linux has no player, so every video
  there fires `error`; on macOS a video's orientation (its track's
  `preferredTransform`) is not applied.
- **UI methods other than `boundingClientRect`, `scrollIntoView`,
  `scrollTo`/`scrollBy`, `selectTab`, a swiper's `scrollTo`, a
  `scroll-view`'s `scrollTo`/`scrollBy`/`getScrollInfo`, a `list`'s
  `scrollToPosition`/`scrollBy`/`getScrollContainerInfo`/`getVisibleCells`,
  an `x-refresh-view`'s `finishRefresh`/`finishLoadMore`/`autoStartRefresh`,
  a scroll coordinator's `setFoldExpanded`, a `text`'s `getTextBoundingRect`,
  an `image`'s four animation methods, a `video`'s `play`/`pause`
  and a dialog's `show`/`showModal`/`close`/`requestClose`.** Those dispatch
  by name (`boundingClientRect`, `scrollIntoView` and CSSOM-View's
  `scrollTo`/`scrollBy` on every element, `selectTab` on
  the two pager tags only, `scrollTo({index, smooth})` on `x-swiper` only,
  the three scroll methods on `scroll-view` only, where they shadow the
  standard two, the five list methods on `list` only, whose `scrollBy`
  shadows the standard one, the refresh view's three on `x-refresh-view`
  only, `setFoldExpanded` on `scroll-coordinator` and `x-foldview-ng` only,
  `getTextBoundingRect`, `getSelectedText` and `setTextSelection` on `text`
  only, the animation methods on `image` only, `play` and `pause` on `video`
  only, the dialog's on `dialog` only) through
  `__InvokeUIMethod`; every other name — the base set's `focus`/`blur` (no
  focus model), a `scroll-view`'s `takeContentScreenshot`,
  a `list`'s `getScrollInfo`, a scroll coordinator's
  `getScrollInfo`, a `video`'s `stop` and `seek`, `requestUIInfo`,
  `takeScreenshot` (no pixel readback) and the rest of the per-component
  catalog — answers code 3, `METHOD_NOT_FOUND`. The rect itself ignores
  transforms and never flushes.
- **The text `layout` event.** The per-line ranges `seldon`'s
  `TextBlock::lines` reports have no delivery path.
- **`rpx`-aware view/device policy** and the rest of the `<list>` component
  (scroll and threshold events, sticky cells and snapping; the
  layout mapping and virtualization are UA rules already, and five UI methods
  are built).
- **Animated image frames off macOS.** Only `ImageIO` decodes an animated
  image's frames by index; gdk-pixbuf and the browser's `Image` decoder answer
  the first frame (`docs/picture-source-design.md` step 5 takes up the
  browser). No `region-to-decode` and no `blur-radius` post-processing.
- **The rest of the canvas surface** (`crates/bobcat-core/docs/canvas.md`,
  `crates/bobcat-canvas/AGENTS.md`). Card JavaScript draws with an
  `OffscreenCanvas` 2D context in any realm and shows it in a `<canvas>`
  placeholder; not built:
  - `drawImage` of a network image: `createImageBitmap` takes an
    `OffscreenCanvas`, an `ImageBitmap` or an `ImageData` only, with no
    `Blob` or URL source. It would decode through the platform decoder
    (`vant::ImageDecoder`; `BrowserDecoder` on wasm32) into a
    `bobcat_canvas::Image`, from `createImageBitmap` in
    `packages/bobcat-element/src/canvas.ts` and a host member in
    `crates/bobcat-core/src/canvas/members.rs`.
  - `toDataURL`/`toBlob` on the element and `convertToBlob` on an
    `OffscreenCanvas`: nothing encodes an image. They would read the
    bitmap back as `getImageData` does (`Canvas::read_image_data`) and need
    an encoder; `vant::Platform` has a decoder and no encoder.
  - A main-thread `getContext` on the `<canvas>` element itself: the element
    has context modes `none` and `placeholder` only
    (`crates/terminus/src/html/canvas.rs`); a `2d` mode would bind a
    `bobcat_canvas::Canvas` to the element's picture there and expose it
    through the Element PAPI (`packages/bobcat-element/src/element-papi.ts`).
  - `ArrayBuffer` in a `postMessage` transfer list (`DataCloneError` today)
    and `ImageBitmap` structured clone (transfer-only today): both are
    `packages/bobcat-element/src/transfer.ts` and the QuickJS serializer's
    transfer list (`crates/quickjs-rust-bridge`).
  - `x-canvas`, web-elements' deprecated wrapper: no component is defined
    for the tag (`crates/bobcat-core/src/main/tree/`).
  - The `bitmaprenderer`, `webgl`, `webgl2` and `webgpu` contexts:
    `getContext` answers `null` for them (`canvas.ts`); a context would be
    a new mode of the canvas entry in `crates/bobcat-core/src/canvas/mod.rs`
    over its own `terminus::link::render::canvas::Surface` texture.
  - The context's `filter`, `letterSpacing`, `wordSpacing`, `fontKerning`,
    `fontStretch`, `fontVariantCaps`, `textRendering` and `lang` attributes;
    `small-caps` parses and is not drawn. They are `bobcat_canvas::Canvas`
    state (`crates/bobcat-canvas/src`) with their accessors in
    `canvas.ts`.
  - Bitmap color glyphs (`sbix`/`CBDT` emoji), which draw nothing
    (`crates/bobcat-canvas`'s text path).
- **Import maps, import attributes, JSON *ESM* modules and Lynx component-bundle
  imports**, and the asynchronous half of the compiled-bundle loader:
  `requireModuleAsync` and `loadScriptAsync`. Lazy containers themselves are
  implemented — `lynx.fetchBundle` plus `lynx.loadScript` on both threads,
  registered by the same `ContainerSources` a page is — and so is
  `require`'s own `.json` parse.

See `docs/tracking/` for the behavior surface each of these is scoped against,
and `.claude/agents/` for the subsystem-scoped agent personas set up for this
work.

See `docs/runtime-architecture.md` for the runtime dependency graph, feature
boundary, private paint pipeline, and frame walkthrough;
`docs/style-architecture.md` and `docs/layout-architecture.md` contain the
style/layout ownership rules.
