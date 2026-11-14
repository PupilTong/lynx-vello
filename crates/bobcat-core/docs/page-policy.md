# bobcat-core: Page policy: tags, text and the UA sheet

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

Core owns Lynx page policy in its `tree` module — the `page` root tag,
`Viewport`/stylo `Device` construction, the Lynx UA cascade defaults, and the
components the engine defines — while tag vocabulary, handle lifecycle, and the
PAPI member surface live in `packages/bobcat-element`. The module is one file
per tag, each owning that tag's UA rules and its tests: `tree::raw_text`
(generated-content CSS and the rules that dissolve a carrier into the `text` it
is written inside), `tree::text` (the paragraph attribute limits and what may
generate a box inside a run, and a `text` component whose
`CustomElement::invoke` answers `getTextBoundingRect` with the per-line boxes
`Document::text_range_boxes` reads off the committed paragraph, in viewport
px, and, for a `text-selection` text — `user-select` in its UA rules, so
a pointer selects it without focus — `getSelectedText` and
`setTextSelection` over the document's selection),
`tree::image` (the `src`-to-replaced-content
reflection and its UA box), `tree::scroll_container` (`scroll-view` and
`list` as scroll containers — which axis scrolls, which one clips, and which
way the subtree stacks, from `web-elements`' own `scroll-view.css` and
`x-list.css`; `enable-scroll="false"` leaves the box a scroll container only
script can move; `initial-scroll-to-index` by the viewpager recipe, a typed
`attr()` and an `if()` over `sibling-index()` into each child's
`scroll-initial-target` — and a `scroll-view` component whose `CustomElement::invoke`
answers the `scrollTo`, `scrollBy`, `autoScroll` and `getScrollInfo` UI
methods on the axis the tag scrolls; `takeContentScreenshot` is not built —
and fires `scrolltoupper`/`scrolltolower` from the `scroll` it observes,
native's edge rule over `upper-threshold`/`lower-threshold`, the
`ScrollEdges` `list` shares, and `nearest_snap_position`, the nearest snap
position the swiper and the pager report by),
`tree::list` (`list`'s scroll axis, `list-type` layout modes, cell
virtualization as UA rules, `initial-scroll-index` as `scroll-view`'s
child rule over a `--list-initial-index` hint the component writes once, at
connect, and a `list` component whose `CustomElement::invoke`
answers `scrollToPosition`, `scrollBy` and `autoScroll` (`scroll-view`'s, on
the list's axis), `getScrollContainerInfo` and `getVisibleCells`;
`getScrollInfo` is not built, and the base set's `scrollTo` answers on a list;
it fires `scrolltoupper`/`scrolltolower` by `scroll-view`'s rule, its zones
also by `upper-threshold-item-count`/`lower-threshold-item-count`;
`scrollstatechange` is not built),
`tree::base_methods` (the UI methods every element has, which
`callElementMethod` runs when the element's kind has no method of the name:
`boundingClientRect` over `Document::bounding_client_rect`,
`scrollIntoView`, CSSOM-View's `Document::scroll_into_view` with
`container: "nearest"`, and CSSOM-View's `scrollTo`/`scrollBy` with a
`ScrollToOptions` dictionary over `Document::scroll_to_with`; `focus`/`blur`
are not built, `terminus` having no focus model), `tree::viewpager` (`viewpager`/`x-viewpager-ng` as a
horizontal scroll container that snaps page by page, its pages' row pinned in
the cascade, the initial page (`select-index`/`initial-select-index`) as a
typed `attr()` and an `if()` over `sibling-index()` into
`scroll-initial-target`, and a component that answers the `selectTab` UI
method, its `CustomElement::invoke`, and fires `offsetchange`, `change` and
`willchange` from the pager's own `scroll`/`scrollend` and the touches on
it), `tree::touch_count` (the count of fingers down per element a component keeps
from the `touchstart`/`touchend`/`touchcancel` its element hears, counted once
per event outside the capture pass: the `isDragging`/`isDragged` of the
swiper, pager and refresh-view events, web-core's approximation of native's
drag state), `tree::swiper` (`x-swiper`/`x-swiper-item`: the first
component with a UA shadow tree, web-core's `#content` scroll container
snapping item by item on its main axis with the items slotted into it, and
`#indicator`, one box drawing the dots as background layers whose active
layer a `steps()` animation on `#content`'s scroll timeline moves, sized by
the item count the component keeps in `--swiper-count` on `slotchange` at its
slot; the five `mode` layouts with `coverflow` and `carry` as
compositor-exported `view()` transform animations — `carry` a scale,
`coverflow` a turn drawn with 2D functions (scale, skew, translate) — and
`current` by the viewpager recipe; `autoplay`, `interval` and `smooth-scroll`
as a timer of the component's — kept in the document's `Autoplays`, which the
page waits out beside its realm's timers (`RealmOwner::next_deadline`) and
runs in its epilogue, each turn a `Document::scroll_to_yielding` request the
painter declines while a pointer holds the swiper; `scrollstart`, `change`,
`scrollend` and `transition` from `#content`'s `scroll`/`scrollend` and the
touches on the host; its
one UI method, `scrollTo({index, smooth})`, scrolls the item into view at its
snap position through `Document::scroll_into_view`, its `CustomElement::invoke`), `tree::refresh_view` (`x-refresh-view`/`x-refresh-header`/
`x-refresh-footer`: a component with web-core's UA shadow tree, a `#container`
scroll container snapping `y mandatory` with 30% placeholders above and below,
the header and footer in named slots (the component sets their `slot` on
`slotchange` at its default slot) and the content in `#content`, the only
resting snap target and the initial scroll target; the header's and footer's
`scroll-snap-align` is a `view(block)` animation, `start`/`end` from 90%
visible, in place of web-core's `x-magnet-enable`; `enable-refresh`/
`enable-loadmore="false"` hide an end; `headershow`, `headeroffset`,
`footeroffset`, `headerreleased`, `footerreleased`, `startrefresh` and
`startloadmore` from `#container`'s `scroll`/`scrollend` and the touches on
the view, by native's `pullingPercent`; three UI methods, its
`CustomElement::invoke`, each a `Document::scroll_into_view` in
`#container`: `finishRefresh` and `finishLoadMore` smoothly put `#content`
back at `block: center`, `autoStartRefresh` instantly puts the first header
at `block: start`, since from rest the header is no snap position and the
painter would settle a smooth request back to `#content`),
`tree::scroll_coordinator` (the ten
`scroll-coordinator`/`x-foldview-*-ng` tags as a collapsing header: the
coordinator a vertical scroll container, the header and slot absolutely
positioned with the slot placed and sized by `anchor-size()` of the header and
the toolbar, a sticky toolbar, `scroll-capture-y: nearest forward` inside the
slot for the fold order, and the structure pinned in the cascade; a component
on the two coordinator tags whose only member is the `setFoldExpanded` UI
method, its `CustomElement::invoke`, a scroll of the coordinator to `offset`
clamped to the fold range; iOS's `getScrollInfo` and `scrollBy` are not
built), HTML's `<dialog>` and popover UA rules, which are `terminus::link::html`'s
(`new_document` calls `Document::define_html_elements` before it defines
the overlays, which build a `dialog` in their shadow trees, and registers
the shared `ComponentEvents` queue `tree/lib.rs` defines as the document's
`QueuedEventSink`, so a dialog's non-bubbling `close` and `cancel` land on
it in order with every component's events, beside an image's `load`/`error`,
for the page's epilogue to post once per batch; `ua_sheet.rs` imports their
rules and writes the display
`defaultDisplayLinear` picks for `dialog` and `dialog:popover-open`; the
dialog's four methods are UI methods, an `InvalidStateError` answering
native's code 7, a ruled deviation from web-core's 4; `dialog_tests.rs` and
`popover_tests.rs` test both under the Lynx sheet), `tree::overlay` (`overlay`/`x-overlay-ng`, Lynx's modal
layer, built as web-core builds it: the host is `display: contents` and
paints nothing, and a UA shadow tree `dialog#dialog[part=dialog] >
div.overlay-inner > slot` carries the content — the dialog
(`popover="manual"`) is `terminus::link::html::dialog`'s own element, driven only through
HTML's API for it, as web-core drives its own: `visible` (present and not
`"false"`) on a connected host shows it with `showModal()`, or — under
`events-pass-through` — as a Manual popover, the top-layer element HTML does
not make the document inert for, and hides it with `close()` or
`hidePopover()`; hidden is therefore the dialog's state no author rule on the
host can override, and what fills the viewport is the transparent dialog,
which an author background on the host never reaches; UA rules keep an
author-displayed host zero-sized, place the first child at the viewport's
top-left, and hide every other child with one `!important`; the shadow sheet
makes a pass-through dialog `pointer-events: none` (its popover `::backdrop`
already is); non-bubbling `showoverlay`/`dismissoverlay` at the host on the
same `ComponentEvents` queue, on transitions only, while the dialog's own
`close`, not composed, stays in its shadow tree; no UI method),
`tree::markdown` (`markdown`/`x-markdown`, web-core's `x-markdown`: a
component that renders `content` into a UA shadow tree
`div#markdown-root.markdown-body[part=root]` of markdown-it's HTML-named
elements — `parse.rs` replicates markdown-it 15 as web-core configures it
over `pulldown-cmark` and `linkify`, `style.rs` turns `markdown-style` into
a second shadow sheet the component replaces in place with
`Document::replace_shadow_stylesheet`, `build.rs` builds the elements,
text holders as `display: block` (each one standard inline formatting
context) with phrasing elements as `display: inline` boxes in it, blocks as
column flex boxes, list markers as
absolutely placed glyph elements, tables as grids; a render reparses and
keeps the elements of the equal prefix of top-level blocks; bubbling
`parseEnd` per render, and `link`/`imageTap` from a `tap` it observes at its
shadow content, the anchor found from the character the tap hits; UI
methods `getContent`, `getImages`, `getParseResult`, `getTextBoundingRect`
and `getCharIndexByPoint` in web-core's rendered-text offsets, which
`rendered.rs` counts with markdown-it's line breaks between block tags; the
typewriter, truncation and selection are deferred),
`tree::frame` (`frame`, web-core's nested `lynx-view`: a component whose
`constructed` attaches an open shadow root holding the nested page
`page[part=page]`, which the UA sheet sizes 100% of the frame and which
belongs to the card rendered under it (`reset_frame` frees it and builds the
next card's) — no shadow sheet and no slot, so light children never render — whose `connected_callback`/`disconnected_callback` queue
`ComponentEvent::Frame`, which the runtime dispatches at the frame as the
internal `bobcat:frameconnected`/`bobcat:framedisconnected` events,
`display: flex; contain: strict` whatever `defaultDisplayLinear` says,
relaxed to `inline-size layout paint style` by `auto-height` and to
`layout paint style` by `auto-width` (UA attribute selectors; `preset-*`
and `enable-multi-async-thread` are accepted and do nothing), the shared
box block, `is_frame`, `nested_page`, `reset_frame` and `frame_depth`; `src`, `data` and
`global-props` are ordinary attributes to Rust, which observes nothing —
`bobcat:frame` reacts to them in the realm that wrote them), and
`tree::blur_view` (`blur-radius` reflected into a
`backdrop-filter: blur()` presentational hint, under both the native tag
`blur-view` and web-core's `x-blur-view`, as a CSS length rather than
web-core's `parseFloat` — the one tag module with no UA rules of its own,
since a blur view is a container and nothing else), and `tree::text_field`
(`x-input`/`x-textarea` and their `-ng` aliases, the four Lynx tags only —
the HTML names `input`/`textarea` are not defined, an HTML-named element
following HTML (user ruling 2026-10-19) — `docs/editable-text-design.md`
E5: a UA shadow tree of one `contenteditable="plaintext-only"` editor — for
`x-input` an engine-sheet `display: block` `div`, its own inline formatting
context, `white-space: pre`; for `x-textarea` a Lynx `<text>`, whose Lynx
paragraph lays the lines out — and a `#placeholder` shown while the editor
is `:empty`, whose text block is the field's `::placeholder` (it carries
`pseudo="-webkit-input-placeholder"`, `terminus::link::PLACEHOLDER_PSEUDO_ID`, so an
author's `.hint::placeholder` rule reaches it under css-pseudo-4's property
restriction, and the UA sheet's `x-input::placeholder` rules lay it out);
the host is the field's box; the placeholder style attributes
as UA `attr()` custom properties; `value`, `type`, `confirm-type`,
`disabled`/`readonly` and `input-filter` mapped onto the editor in its
attribute callback; line breaks (single-line), `maxlength`, the `number`
filter, `input-filter` and the textarea's `maxlines` (the longest prefix of
the insertion whose value lays out in the limit, by bisection with a layout
pass per probe over `Document::paragraph_line_count`) corrected after each
edit with `Document::set_host_value`; web-core's
`input`/`confirm`/`selection`/`focus`/`blur` re-fired at the host from the
editor's standard events, heard at its shadow root, on `ComponentEvents` with
string extras; and web-core's methods, its `CustomElement::invoke`).
`tree::ua_sheet` owns what
those tags agree on, the order
they cascade in, and `PageConfig`; `tree/lib.rs` only mints the document they
describe. Attribute policy stays in each tag's module. Numeric text and list
attributes use UA `attr()` declarations and registered custom properties;
`tail-color-convert` uses an attribute selector. Only image resources, blur
hints, the swiper's item count, the refresh view's slot
assignment, the dialog's `:open`/`:modal` state, the overlay's shadow
tree, its dialog's top-layer membership and its events, the markdown
element's shadow tree, its rendering and its events, the frame's
shadow root, and the list's `initial-scroll-index`, read once at connect, need `terminus::link::CustomElement` callbacks. Runtime attribute members perform
DOM mutations; Stylo tracks attribute dependencies and recascades on changes.
HTML's own rules — the dialog's, the popover's and the display `dialog` maps
`block` to — are written with their selectors inside `:where()` and ordered
so that source order is HTML's precedence among them: every Lynx rule names a
tag and so outranks them all, as web-elements' author rules outrank a
browser's UA sheet under web-core (a `<view popover>` keeps its Lynx
`display`). That is a cascade layer by hand: an `@layer` in a UA sheet trips
Stylo's rule-tree ordering assertion, whose root node carries an unlayered UA
rule's priority (`docs/style-assumptions.md` §31).
The UA assembly order is mostly documentation, with one exception that is
mechanism: `image`'s and `svg`'s child suppression ties on specificity with the
`display` rules `view`, `scroll-view`, `list`, `blur-view`, `x-blur-view` and
`wrapper` carry, so it wins only by being assembled last.

`tree::svg` is the Lynx `<svg>` component
(`docs/svg-lynx-component-design.md`): one picture, never a subtree, as
web-core's `x-svg` and native draw it. `src` goes to
`Document::set_image_source` (the host fetches the document and reports its
bytes); `content` goes to `Document::set_image_document` as its bytes, filed
under a synthetic `svg-content:<128-bit hash>` source that is parsed by the
engine, once per distinct markup, and never asked of the host. An empty or
removed `content` keeps the current source (web-core's `_handleContent`); the
last attribute written wins. UA rules `svg { display: flex; }` and
`svg > * { display: none; }`, with `svg` in the shared box block: no size
containment, so an unsized `<svg>` takes its natural size. Its `load` carries
the element's border-box layout size read at delivery (native's detail, 0x0
without a box; `dispatch_component_events` tells it apart with
`tree::is_svg`), and a failed source dispatches nothing (web-core fires no
`error`). Because the detail is read at delivery, the epilogue holds the whole
component-event batch while its commit was skipped (`needs_render`, a listed
author sheet still outstanding) and posts it after the first commit.

The native host-module functions call `terminus::link::Document` directly. Element
identity is the DOM `NodeId`, which is also the element's Lynx `unique_id` —
one number, issued by the DOM, never reissued; the JS side mints no ids.

**Text** reaches the engine as an attribute and becomes generated paragraph
content. Script writes `__CreateRawText(value)` — a `raw-text` element carrying
`text` — and its UA rule `raw-text { content: attr(text); }` feeds the same DOM
generated-content path as author CSS; `text[text]` uses the same rule. There is
no custom element reflection or synthetic DOM text child. Content replaces
rendered children while preserving DOM structure. The element's primary text
style shapes and paints its run; attribute changes invalidate the paragraph,
and unchanged text/style reuse its shaping. `text` establishes one flattened
paragraph whatever `defaultDisplayLinear` says, `wrapper` is
`display: contents`, and `raw-text` dissolves into the `text` it is written
inside, or — a ruled deviation from web-core — into a tag that is not a Lynx
tag (`div`, `span`, any other name; `tree::raw_text`'s `LYNX_TAGS`), there as
loose text that `terminus` boxes into anonymous items; under any other Lynx tag it is
`display: none`. One `wrapper` level is looked through either way. A `text`
keeps its text verbatim, as every native backend does (user ruling
2026-10-16): every Lynx paragraph (a Lynx text element's) skips CSS white-space
processing (`seldon::text::paragraph`'s `WhiteSpaceProcessing::Verbatim`; no
switch, user ruling 2026-10-17), so space runs, leading spaces, tabs (20px
stops) and every newline stay, trailing spaces hang, a newline that ends the
text adds an empty last line (Android's `StaticLayout`), and only
`white-space: nowrap` stops the wrapping, showing nothing after the first
newline — native parses no other keyword, so `pre` and the rest wrap.
`<markdown>`'s paragraphs are standard inline formatting contexts, not Lynx
paragraphs, and keep css-text-3 collapsing. The carrier's own
`white-space-collapse: preserve-breaks` decides its newlines only as loose text
in an HTML element (the standard inline formatting context). Sibling runs and nested text share the establishing
element's paragraph. `text { color: initial; }` stops an ancestor's color at the text
root and `text > text` inherits it again, as in web-elements; a page whose
config says `enableCSSInheritance: true` gets one more UA rule,
`view > text, view > wrapper, view > wrapper > text { color: inherit;
text-decoration: inherit; }`, which is web-core's rule for that switch
(lynx-stack #3907) reduced to the two declarations that change a value here
(`tree::text::CSS_INHERITANCE_RULES`, `docs/tracking/deviations.md`). UA
`attr(... number)` declarations supply `--lynx-text-maxline` and
`--lynx-text-maxlength`; `<integer>`, non-inherited registrations reject
fractional counts and default to `-1` (unlimited). No numeric-prefix parsing or
CSS expressions in attributes are supported. `text[tail-color-convert="true"]`
sets `--lynx-tail-color-convert: 1`, otherwise its registered default is zero.
A `text > inline-truncation` subtree carries `--lynx-inline-truncation` so
`crates/terminus` can lay it in at the clamp without naming a Lynx tag.
DOM's borrowed `StyleView` reads their computed values
through `LynxTextStyle`, and `BlockStyle::from_container_style` consumes
those inputs. Normal and animated style refreshes merge effective limit changes
into layout damage to re-break the retained glyphs through existing box
invalidation. Original attribute strings remain available to selectors. No text
custom element, separate paragraph-limit storage, or public limit setter
participates. Computed `text-overflow` selects clip or the literal-dots
ellipsis. The text `layout` event remains unwired.
`docs/text-measurement-and-ifc.md` records the earlier design investigation;
current integration status lives in `docs/tracking/css-text.md`.
