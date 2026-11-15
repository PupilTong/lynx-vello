# crates/terminus

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/terminus`. Paths in backticks are relative to the repository root.

Generic W3C-DOM-subset document tree and standards-oriented CSS computation
core, on stylo's cascade. `docs/terminus-link-api.md` is the authoritative
normal-build versus test-feature API boundary. It must not contain Lynx
runtime-element vocabulary or Lynx device/unit policy: Lynx computed defaults
(border-box, `overflow: clip`, `display: linear` on every element, …) stay
embedder cascade policy in the UA sheet.

Its one public module is `terminus::link` (`src/link.rs`). Every other module
is private and the crate root re-exports nothing, so every crate built on it —
the bobcat crates, `prime-radiant`, `seldon`'s benchmarks, and `terminus`'s own
integration tests, benchmarks and examples — names an item as
`terminus::link::…` (`terminus::link::Document`,
`terminus::link::painter::Painter`, `terminus::link::html::dialog::show_modal`).
The link re-exports each item by name, in submodules that keep the crate's
module names (`editing`, `event`, `input`, `layout`, `render`, `scroll`,
`html`, `painter`) with everything else at its top, and
`#![deny(unreachable_pub)]` keeps it curated: a `pub` item the link does not
reach fails the build, so it is on the link (a consumer names it, or a link
item hands it out) or it is `pub(crate)`. A new consumer need is a new name on the link and in its row of
`docs/terminus-link-api.md`.

Subsystems:

- `tree/` — the boxed `TreeArenas<T>`, `Node`, `Document`, shadow roots, the
  flat tree, custom-element definitions and reactions, the document's
  top layer with its `::backdrop` nodes (`tree/top_layer.rs`), and the
  anonymous block boxes around loose text (`tree/anonymous.rs`): each
  contiguous sequence of text among the children of a `flex`, `grid`,
  `grid-lanes`, `linear` or `relative` element — text nodes, and
  `display: contents` elements whose `content` replaces their children —
  becomes one item, a detached node with the internal block container
  display; a white-space-only sequence becomes none (css-flexbox-1 §4,
  css-grid-1 §6). `LayoutTree::children` answers the boxes in place of the
  text, they are re-derived after each style flush from what mutations
  recorded, and a hit on one targets its container.
- `html/` — HTML elements `terminus` implements over its own tree, never
  defined by `Document::new` (a test that builds a `<dialog>` by hand gets a
  plain element) but by `Document::define_html_elements`, which an embedder
  calls before any element carries their tags: `html::dialog` is HTML's
  `<dialog>` — `UA_RULES` and `DIALOG_TAG`, a component with no per-element
  state whose `open` attribute callback is the one path that flips `:open`
  and, on removal or `"false"`, leaves the top layer, "modal" being
  `Document::blocks_document`, and `show`/`show_modal`/`close`/`request_close`
  (also the component's `CustomElement::invoke`, an `InvalidStateError`
  answering `MethodError::InvalidState`); `html::popover::UA_RULES` is HTML's
  popover rules over `tree/popover.rs`. Every rule is HTML's selector inside
  `:where()`, `display` is the embedder's to write, and the module doc gives
  the source order that keeps HTML's precedence. A dialog's `close` and
  `cancel` go, at the moment they are queued, to the document's
  `event::QueuedEventSink` (`Document::set_queued_event_sink`, registered
  once by the embedder; with none they are dropped).
- `style/` — the per-document `StyleEngine` (`Stylist`, cascade pipeline,
  device, stylesheet set, the document's own `SharedRwLock` for its author
  sheets and inline styles, and the one process-wide lock every user-agent
  sheet is parsed under, which is what lets `UserAgentStylesheet` be parsed
  once and mounted by every document), flush, invalidation, `StyleDamage`.
- `layout/` — the concrete `seldon` host: `Document::layout`, the `LayoutTree`
  impl, per-node `LayoutSlot`s in `DocumentLayoutState`, and the two
  per-element facts a `StyleView` answers with that no computed value carries:
  `content-visibility: auto` relevance (`relevance.rs`) and the css-sizing-4
  last remembered size (`remembered.rs`). Two kinds of box lay their content
  out as one paragraph: a Lynx text element (`Node::is_lynx_text`, from its
  custom element definition, under any `display` but `none` and `contents`;
  the fork's `display: -lynx-text` value that did this until 2026-10-17 is
  removed) (`text_block.rs`, the flattened Lynx paragraph) and the
  `(block, flow)` block container
  (`DisplayMode::BlockFlow`, `inline_flow.rs`, a CSS inline formatting
  context; no author style produces it — `block` and `inline` parse only in
  engine sheets — and its producers are the anonymous block box around loose
  text and engine-built HTML content). Its content is text runs, non-atomic
  inline boxes (`inline_flow::is_inline_box`: a `display: inline` element
  whose box parent is a block container) with the full inline box model under
  `box-decoration-break: slice`, and atomic inline boxes — a replaced element
  and every block-level box, block-in-inline splitting being a recorded
  deviation. An inline box generates no layout box: the paragraph's fragments
  are its geometry, the painter draws its background and border from them
  (`paint::walker`, `paint_inline_boxes`), the paint-order build gives each a
  hit-only `PaintItemKind::InlineFragment` record so a hit on it targets the
  box, and `Document::bounding_client_rect` unions them. `paragraph.rs` holds
  what both kinds share: the `TextStore` a node's layout state keeps, either
  kind, the probe/restore routing, the measure closure, the
  `PaintedParagraph` the painter reads, and `paragraph_of` /
  `inline_source_range`, which the geometry APIs (`text_range_boxes`,
  `text_index_at_point`) answer from for either kind; `paragraph_kind` and
  `paragraph_line_count` say which kind a node established and how many
  lines its last commit laid out. The `<text>` `layout` event's `terminus` half
  (`docs/layout-observer-design.md` §3.3): `Document::set_text_layout_reporting`
  puts a Lynx text element in `DocumentLayoutState::text_layout_reporters`
  (a node-keyed set, empty and unallocated on a page without one; a
  non-Lynx-text node panics); the committing branch of
  `compute_text_block_layout`, reached only on a seldon cache miss, pushes a
  reporting element onto `text_layout_reports`;
  `Document::take_text_layout_reports` hands that list over deduplicated in
  first-occurrence order (one `layout()` can commit a paragraph in several
  passes), an empty `Vec` without allocating when there is none; a freed node
  leaves both. `Document::text_lines` answers the last commit's lines as
  `TextLineReport { start, end, ellipsis_count }` (source UTF-16 units, from
  `TextBlock::lines`). The host side (listener gate, component event, detail)
  is not built yet.
- `visual/` — stacking contexts, CSS2 Appendix E paint order, transforms,
  `RenderLayer` group effects, reverse-paint-order hit testing, and
  `content-visibility: auto` relevance determination (`relevance.rs`), which
  runs inside `render` between the paint-order build and the walk.
- `paint/` — the document-owned private `FramePainter`, walker, fragment painters
  and the display list they record into (`paint/display_list.rs`: fills,
  strokes, clips, layers and glyph runs with their bounds), whose fragments a
  `commit` publishes in its `CommittedFrame`.
- `scroll/` — CSSOM-View geometry, per-node offsets, `scroll_to`/`scroll_by`/
  `scroll_chain`, and the one chain walk (`drive_chain`) both the document
  and the presenting painter (`painter/`) run: `overscroll-behavior` fences the reach,
  the engine's own `scroll-capture-x` / `scroll-capture-y` (shorthand
  `scroll-capture`) hands a gesture to the container above first (per axis,
  for the named delta sign). `scroll/snap.rs` is css-scroll-snap-1 — positions from
  `scroll-snap-type`/`-align`/`-stop`, `scroll-padding` and `scroll-margin`,
  published per scroll slot, settled on at a drag's end, stepped to by a
  wheel tick, and re-snapped at rest on every commit; no snap events. Each
  position names its snap area, so a container that stood on an area's
  position follows that area through a layout change (§4.1.3) instead of
  taking the position nearest its old offset: the document moves its own
  offset inside the render (`Document::resnap`, the committed frame's
  positions against the newly built ones), and the presenting painter the
  offset it holds when it adopts the commit, unless a drag, fling, bounce
  back or glide has the container.
  `scroll/request.rs` is CSSOM-View's `scrollTo({behavior})`: a script-facing
  scroll is a request the committed frame carries to the painter, which
  animates a smooth one and names the request in the offsets it posts back.
  `Document::scroll_to_yielding` is the one request the painter may decline,
  while a pointer holds the container, for a turn a timer asked for rather
  than a script (an `x-swiper`'s autoplay); the document moves no offset for
  it. Lynx's `autoScroll` rides the same file as `Document::auto_scroll`: a
  change of a container's constant-rate motion, kept in a list that is empty
  unless a script asked for one and carried beside the frame's scroll slots
  rather than in them, so it costs a container that never auto-scrolls
  nothing and arrives together with a scroll request of the same turn.
  `scroll/into_view.rs` is CSSOM-View's `scrollIntoView(options)`
  (`Document::scroll_into_view`, the editor's draft with its `container`
  option): each scroll container on the element's containing-block chain,
  innermost first, gets such a request to the position `block`/`inline`
  choose against its snapport and the element's `scroll-margin`.
  `scroll/initial_target.rs` is css-scroll-snap-2's `scroll-initial-target`:
  the build records the `nearest` elements, the render scrolls each
  container to its first one — `scrollIntoView`'s position with
  `block: start`, `inline: nearest` — as an instant scroll request and
  rebuilds the frame in the same commit.
- `editing/` — HTML `contenteditable` plain-text editing
  (`docs/editable-text-design.md`): editing hosts, the one focused element
  and its queued focus events, a parley `Selection` and an IME composition
  over the committed paragraph that lays the host's text out — a Lynx text
  element's own, a block container's own, or the anonymous block box around
  a flex host's loose text; a host's own paragraph keeps one empty line of
  its strut when empty, so the host is one line tall — with every offset
  converted through that paragraph's source map
  (`seldon::text::paragraph::OffsetMap`), the
  Chromium-shaped command set with
  its `beforeinput`/`input`/`composition*` events reported as data
  (`prepare_editing`/`commit_editing`), `TextInputState`, and the read-only
  selection of an element whose used `user-select` is not `none`
  (`is_user_selectable`, `selectable_root_of`, `begin_selection`). The caret, selection highlight
  and composition underline are painted by `paint/text.rs`. `bobcat-core`'s
  `main/runtime/editing.rs` dispatches the events and runs the default
  actions (E2); the events cross as `EventDetail::{Input, Composition,
  Focus}`. The element layer over it (E5, `tree::text_field`) writes a value
  with `set_host_value` (no event, the composition ended, the selection
  placed) and the host's `type` attribute picks `TextInputState::input_type`;
  `has_focus_events` says a method's focus change is waiting. The document's
  selection is the focused host's (`selection()`, anchor and focus in UTF-16
  units, `None` with nothing focused), and every change of it — focus, blur,
  the fixup rule, an applied command that moved it, a value write — sets the
  Selection API's "has scheduled selectionchange event" flag
  (`has_pending_selectionchange`/`take_selectionchange`).
- `input/` and `event/` — the `InputEvent` host seam, `Document::event_steps`,
  which computes a path for a *script* dispatch above,
  `Document::dispatch_element_event_over`, which walks those same steps here
  first, delivering to defined `CustomElement`s through `handle_event`, and
  `Document::dispatch_element_event`, the same walk for an event only the
  engine hears. `event::EventDetail` is the one description of an event's
  payload; `CustomElement::observed_events` gates the walk.
- `render/` — the DOM-free floor absorbed from the former `pulsar` crate
  (2026-08-04): `FrameImages`, the `render::gpu` wgpu backend
  (`SharedGpu`: one device, one `vello_gpu` renderer and one bitmap table
  shared by every target in a process), and the rasterizer `render::raster::Raster`,
  which replays a display list into `vello_gpu` (`render::sparse`), drawing
  glyphs as outlines (glifo's glyph atlas cache is off). A bitmap paint is an
  `ImageRef`: the `ImageDescriptor` an upload is keyed on, and pixels that are
  held, lent (a weak blob — what a painter's per-commit table hands a composed
  list and a filter bake, so neither keeps a bitmap alive) or only uploaded;
  the pixels are read on an upload miss alone. `render::sparse::BitmapLeases`
  is one owner's hold on uploads that does not depend on having drawn them
  recently, and the budget trim skips a leased upload as it skips a pinned
  one. `render::picture` is what an image draw names: a `PictureKey`, the
  URL the page wrote or a process-unique `PictureId` one element owns, and the
  `PictureSlot` a producer on any thread publishes an `Id` picture's newest
  frame into. `FrameImages` answers a generation per picture that changes
  exactly when its pixels do, takes an `Id` picture's frame out of its slot
  (`take_frame`), and names the platform frame type it would hand over
  (`PlatformFrame`; the painter accepts `Infallible` only, as no writer for a
  platform frame exists). The painter writes a taken frame into the picture's
  own texture in the bitmap table (`BitmapLeases::write_picture`): one texture
  and one texture id per picture, written in place, reallocated under the same
  id when the size changes, never an atlas (`vello_gpu` 0.4's atlas holds
  glyphs only). `render::blur`
  is the one exception: a `filter: blur()` bake is a partial replay of a
  committed frame's compose program, so it reads `CommittedFrame`'s filter side
  table, while still naming no node, style, layout or paint-order type. A
  `backdrop-filter` bake at the document's root renders over the canvas
  colour its caller composites over, which `FilterTextures::prepare` and
  `Headless::prepare_filters` therefore take.
- `painter/` — the presenting painter (`link::painter::Painter`), built on the
  thread that draws and `!Send`: the input router and gesture synthesis on an
  owner's `GesturePolicy` (`gesture.rs`), the scroll kinematics (`motion.rs`)
  and the drags, flings, bounce backs, glides and auto-scrolls run over its
  scroll intents (`inertia.rs`), the pixels an adopted frame draws
  (`images.rs`) and the window target (`graphics.rs`). Its compose key is
  the commit, the scroll generation and the revision of what the commit's
  image draws resolve to, and a `filter`/`backdrop-filter` bake keys on the
  bitmaps its own range draws (`FilterGroup::pictures`), so a picture's new
  frame or a restore recomposes with no commit and re-bakes only the bakes
  that draw it; each owes one frame, and a picture's known next change is
  reported (`Painter::next_picture_change`) rather than counted as animating. It composes each
  committed frame at its own offsets and posts them back
  (`Painter::drain_changed`); `bobcat-core`'s `Painter` facade is its link to
  one view.

**Vector images** (`docs/svg-lynx-component-design.md`): an SVG document used
as an image — `<image src>`, `background-image`, `mask-image`, the Lynx
`<svg src|content>` — reaches the document parsed. The host parses it, where
it decodes bitmaps, with `ImageEvent::parse_document(source, bytes, kind)`
and reports `ImageReports::parsed_document(source, Arc<VectorDocument>)`,
carried by `ImageEvent::ParsedDocument`, or `failed` (`DocumentKind`, what
the host passes the parser, is `#[non_exhaustive]`, so a second engine-drawn
format is one more variant). The parse is `terminus`'s own
converter, `render/svg/` over `roxmltree` and `svgtypes` (no `usvg`; its
module docs list the supported subset). It reads presentation attributes
only: CSS inside an SVG document (a `<style>` element, a `style` attribute)
is not read, by ruling, because native's ServalSVG has no `style` element
and the project keeps stylo as its one styling engine. `svg::parse` produces a
`VectorDocument`, a flat paint-order command list with geometry, transforms
and brushes resolved and text collected but unshaped, so it needs no fonts and
is `Send + Sync`; it is `pub` with no public member, and shared in an `Arc`
so every report of one parse (one per view waiting on a load) holds one copy.
`ImageEvent::parse_document` is the only entry to the converter from outside
`terminus`, and returns `ImageEvent::ParsedDocument` or `Failed`.
`Document::apply_image_events` never parses. Applying a parsed document shapes its `<text>` through the document's own
`TextContext` (`seldon::text::shape_line`; font family, weight, style,
stretch and size come from the SVG's own attributes, not the host element's
style) and records it once into a crate-private `VectorImage` (an
`Arc<DisplayList>`, its viewport, its `preserveAspectRatio`, a process-unique key),
kept in `ImageState::Ready { width, height, kind: ImageKind::Vector(..) }`
with the document's natural size in whole CSS px (at least one per axis). Every
layer the converter opens is `Normal`: no blend can be set without CSS. A later
`@font-face` does not re-shape already-encoded text.

The frame carries the scene, never pixels: each visible tile of a vector
image becomes a `VectorDraw` in `Presentation.vector_draws`, referenced by
`ComposeOp::Vector` (`paint/background.rs`, `paint/compose.rs`), so
`FrameImages` never sees one and the monolithic `Document::scene()` draws
none. The painter's raster cache (`render/vector_textures.rs`,
`VectorTextures`, owned beside `FilterTextures`) bakes each draw with
`Raster::render_to_texture` at its device size, keyed by `(image key, width,
height)`, binds the premultiplied texture under a texture id
(`Raster::bind_texture`) and replays the op as a draw of that texture;
resident textures are bounded by `MAX_VECTOR_TEXTURE_BYTES` (64 MiB native,
32 MiB wasm32) with the frame's draws pinned and the rest evicted
least-recently-used. `Headless` bakes the same way, so goldens exercise it. Masks, filters, patterns, markers and
nested `image` elements draw nothing.

**Replaced boxes and the URL image service.** A replaced box presents one
picture at one natural size: `Document::set_picture(node,
Option<(PictureKey, Option<(u32, u32)>)>)`, read back with
`Document::picture`. It is the one replaced-content primitive besides
`set_natural_size`; the paint walk draws the picture into the content box,
fitted against that natural size, resolving a `Url` against the registry as
it resolves a CSS image, so a pending or failed URL draws nothing. Which
picture an element presents, and when that changes, is the embedder's:
`terminus` has no `src`, no `placeholder`, no choice between two sources and
no per-element `load` or `error` (`bobcat-core`'s `tree::image_sources` holds
those for `<image>` and `<svg>`). What it offers instead is the registry as a
URL image service with no element in it: `Document::request_image(url) ->
ImageStatus` asks for a URL once (`take_wanted_images`) and answers what is
known (`Pending`, `Bitmap { width, height }`, `Vector { width, height }`,
`Failed`); `image_status` reads it without asking; `apply_image_events`
answers each `(source, ImageStatus)` a batch of reports settled, which is how
an embedder learns an outcome. A settled status never moves, and a host URL
is never forgotten.

**Documents handed over as markup.** `Document::insert_image_document(bytes,
kind) -> Arc<str>` files a document the page wrote itself (the Lynx
`<svg content>`) under a synthetic source, `svg-content:` and the 32 hex
digits of a 128-bit SipHash-1-3 of the bytes (`siphasher`), and answers that
source for the caller to present; identical markup is one entry, one parse,
one scene and one texture however many elements present it. An unknown
source is created `Pending` and its bytes queued as a document request,
`Document::take_document_requests` (`terminus::link::DocumentRequest` tuples,
drained once, which the embedder hands the host to parse and report like a
fetched document); a known one is answered as any URL is. Nothing ever asks
the host to fetch a synthetic source. A replaced box presenting a URL holds
it (`ImageRegistry::bind`, a count per entry), and a synthetic source is
forgotten when the last box presenting it lets go (another picture, or
`free_node`), taking any unrequested bytes with it — the one removal the
registry makes. The host keeps nothing of a parsed document, so markup that
comes back afterwards is requested and parsed again, and a report that finds
its source settled or forgotten moves nothing. `knows_image_source` is a
test-only registry probe.

Rulings and limits to know before touching it:

- The whole `unsafe` surface is two blocks, each with a `SAFETY` comment kept
  honest by `#![warn(clippy::undocumented_unsafe_blocks)]`.
- A `NodeId` is never reissued (no generation counters, no epoch gate on the
  retained frame) and the document element is permanent and pre-created.
- `overflow: auto` stays out (user decision, 2026-07-29) and a `visible` axis
  pairs into `hidden`; only `scroll` is user-scrollable, `hidden` is a scroll
  container only script moves, and `clip` is no container at all. A scroll
  container is no stacking context by itself, as on the web and web-core
  (user decision, 2026-09-24): native Lynx's `z-index` capping inside
  scroll views is deliberately not followed.
- `content-visibility: auto` relevance is `terminus`'s, determined once per commit
  against the region the paint walk's culling admits, and stored as
  layout-side per-element state in a slot-keyed side table on `TreeArenas` —
  never a Stylo `ElementState`, never a restyle trigger. `seldon` sees it only
  as `CoreStyle::skips_contents`. A relevance flip reveals inside the same
  commit (`crates/terminus/src/visual/relevance.rs`, at most four build passes
  under one commit id), so no frame is published with a reveal pending.
- Animations and transitions inside **skipped contents** are frozen
  (css-contain-2 §4): the driver carries their start times by each tick's
  interval rather than stepping them, the skipping element's own animations are
  untouched, and a frozen set counts as idle for `has_active_animations`, the
  frame's animation flags and so `Painter::owes_frame`. The one deviation —
  style is not skipped, so an animation that *starts* while skipped is created
  and frozen at its start rather than not created — is in
  `docs/style-assumptions.md` §19.
- The css-sizing-4 **last remembered size** is `terminus`'s in the same way, in a
  second slot-keyed side table on `TreeArenas`
  (`crates/terminus/src/layout/remembered.rs`): the layout host records a box's
  content box after every committing run in which it had no size containment,
  and `StyleView` substitutes it into `CoreStyle::contain_intrinsic_{width,
  height}` once the box is skipping. That commit's layout run is the spec's
  recording moment (the resize observations gather after that same layout
  and deliver later, from a posted entry), and csswg-drafts
  #8407 (`content-visibility: auto` implies the `auto` keyword) is folded in
  here because the fork's own adjuster is `#[cfg(feature = "gecko")]`.
  `seldon` is unchanged by either: it reads `AutoLength(l)` as `l` already.
- `position: sticky` stays in normal flow and resolves its inset/containing-block
  constraints in the retained frame at live scroll offsets; painting, hit
  testing and bounding rectangles share the same sticky geometry.
- The crate dispatches no events and has no `preventDefault` and no gesture
  recognizer; `InputEvent::default_prevented` is the embedder's seam.
- Custom elements are user-agent components only, `define` must precede any
  element with its tag, reactions are queued rather than called inline,
  `disconnected_callback` takes a shared `&Document`, `freed` (the id alone,
  called inline from the free of each defined node) is where a definition drops
  state it keys by `NodeId`, and `slotchange` fires at
  the drain boundary (no microtask checkpoint here) to the host, whose
  `handle_event` hears the shadow root a non-composed path ends at.
- Attribute-derived style enters through `Document::set_presentational_hint` at
  `CascadeOrigin::PresHints`, never the author's inline block.
- The fork's `lynx` feature carries css-values-5 `if()` (an arbitrary
  substitution function with `style()` / `media()` / `supports()` tests) and
  `sibling-index()` / `sibling-count()`, neither of which Lynx has
  (user-directed 2026-09-28). `terminus` adds no code for them: a queried custom
  property, an `attr()` read, a child-list change and a device change already
  recascade what depends on them, and the fork flags a style that reads a
  parent's non-inherited custom property `INHERITS_RESET_STYLE`. Scope,
  choices, the Blink differences and the known typed-`attr()` gap are in
  `docs/style-assumptions.md` §27.
- The fork's `lynx` feature also parses css-cascade-6 `@scope` (user-directed
  2026-10-12; Lynx's CSS compiler drops the rule and the `.web.bundle` wire
  cannot carry it, so it reaches the engine through CSS-text sheets only).
  Matching, proximity, invalidation and the sharing cache's scope check are
  stylo's. `terminus` adds one thing: a shadow tree's sheets — CSS text through
  `Document::add_shadow_stylesheet`, pre-built rules through
  `Document::append_shadow_rules` — are
  `ShadowStyleSheet`s (`crates/terminus/src/style/engine.rs`), whose implicit
  `@scope` root is the shadow host (`ImplicitScopeRoot::Constructed`), where
  a document sheet's is the document element. WPT ports are in
  `crates/terminus/tests/scope.rs`; scope and rulings in
  `docs/style-assumptions.md` §33.
- css-anchor-position-1 (the Editor's Draft module, user-directed
  2026-09-29/30; not in Lynx) is `terminus`'s host half: an anchor-name index and
  each element's position options (cascaded at the style harvest with the
  fork's `Stylist::resolve_position_try`) in `TreeArenas`' `AnchorRegistry`,
  the §2.3 target lookup, remembered scroll offsets, the last successful
  option recorded per rendering update, and a settle loop that re-runs
  layout at most once per anchor-positioned box whose read went stale
  (`crates/terminus/src/layout/anchors.rs`). The
  default scroll shift and `position-visibility` are one space-tree node per
  anchored box, sampled at compose and hit-test time
  (`crates/terminus/src/visual/anchored.rs`); a page without anchor positioning
  pays one `is_empty` test per run and per adopted scroll and three field
  reads per restyled element at the harvest. Scope, the "Out"
  list, approximations and gaps: `docs/style-assumptions.md` §28; the
  browser-visible differences: `docs/tracking/deviations.md`.
- css-position-4's **top layer** is an ordered side table on `TreeArenas`
  (`crates/terminus/src/tree/top_layer.rs`), generic rather than dialog-specific:
  the embedder adds and removes entries (`add_to_top_layer(element,
  blocks_document)`, `remove_from_top_layer`), sets `:modal`/`:open` as
  element state, and `terminus` does the rest — `fixed` lowering against the
  initial containing block with a static position of zero, a paint-order
  tail after the root stacking context, the inert floor below the topmost
  blocking entry for hit testing, relevance, and removal with the element
  through the one unlink path. Each entry's `::backdrop` is a detached
  element node (marked by its node's backdrop flag) styled by Stylo's lazy pseudo
  cascade after each flush, laid out by a tail after the rounding tail, and
  hit-tested as its element; it is never handed out. A side table, not a
  per-node bit: the layer is almost always empty and every per-box reader
  tests `is_empty` first. §3.1's computed-value fixups run where a UA rule
  declares the UA-only `-servo-top-layer` (`terminus::link::html`'s `dialog:modal`,
  `:popover-open` and `::backdrop`); membership, not the longhand, stays `terminus`'s truth for the
  containing block (see `docs/terminus-architecture.md` "Top layer and
  `::backdrop`").
- HTML's **popover** attribute is `terminus`'s (`crates/terminus/src/tree/popover.rs`),
  because every element carries it: its four states (`PopoverState`), the
  visibility state as `ElementState::POPOVER_OPEN` (`:popover-open`, which
  only this module writes), and HTML's *show popover* / *hide popover*
  (`show_popover`, `hide_popover`, throwing `PopoverError`) for Manual
  popovers, each a top-layer entry that does not block the document. The
  attribute setters hide a showing popover whose attribute changes state,
  removal included (Chromium's and Gecko's behavior, where the spec's text
  would leave it showing), and the unlink path hides one that left the
  document. A subset of a browser's capability and nothing that behaves
  differently (user ruling): Auto and Hint popovers are refused rather than
  shown, `beforetoggle`/`toggle` are the embedder's to fire, and no script
  reaches a popover (`docs/style-assumptions.md` §31).
- A W3C **IntersectionObserver primitive** (`crates/terminus/src/visual/intersection/`,
  user-directed 2026-10-08, Rust-only): observers carry a
  `Box<dyn IntersectionEventHandler<T>>` (user ruling 2026-10-10: `dyn` for
  simplicity, so one document holds any number of handler types with no new
  bound on `Document<T>`; one `Box` per observer, one indirect call per
  delivery), the node that handler is bound to (the observer is dropped with
  it), and, from one `IntersectionObserverInit`, an implicit or element root,
  a `rootMargin`, a target margin, sorted thresholds, plus ordered targets.
  The target margin is a Lynx-motivated extension the spec does not have: it
  dilates each target's border box in its own space, before its transform
  and every clip, and the entry's rects and ratio are over the dilated rect,
  so a margin reaches an element a clipping ancestor cut, as native
  exposure's grown element rect does; the spec's `scrollMargin` dilates
  scroll containers' clips only and stays out.
  `update_intersection_observations(time)` is §3.2.10 over the last layout
  and the live scroll offsets — run by the host after each
  commit or adopted scroll, gated by a stale bit `render`, a moved
  `scroll_to` and `observe` set — and `notify_intersection_observers` is
  §3.2.5's whole loop, re-entrant: each handler is out of its observer only
  while its own notification runs. `ElementHandler(node)` calls
  `CustomElement::intersections_changed` in its own reaction scope. Geometry includes transforms (the painter's
  `ContextMatrix`), clips through every on-chain overflow-clipping ancestor's
  padding box edge-inclusively (Chromium's `isIntersecting`), resolves
  `rootMargin` percentages by height/width, and reports skipped contents as
  never intersecting; `bounding_client_rect` keeps its transform-free walk.
  A page without observers pays one `is_empty` per freed node and one bool
  per render, moved scroll and epilogue.
- A W3C **ResizeObserver primitive** (`crates/terminus/src/visual/resize/`,
  Rust-only, `docs/layout-observer-design.md` §3.1 and §5): observers carry a
  `Box<dyn ResizeEventHandler<T>>` (the same `dyn` ruling) and the node it is
  bound to, and registrations of a target, an `ObservedBox` (content, border
  or device-pixel content box; horizontal writing mode only) and the spec's
  `lastReportedSizes`, which starts at `-1 × -1` so the first update reports
  every target once. `update_resize_observations` reads the rounded layout —
  `0 × 0` for a target with no box (`display: none` on it or an ancestor,
  detached, no layout) — and queues an entry carrying `contentRect` and all
  three sizes wherever the watched size moved. A target whose contents an
  ancestor skips (`content-visibility`) is left alone, neither compared nor
  updated, so it reports only a size different from its last one once it is
  laid out again: Chromium's `ResizeObservation::ObservationSizeOutOfSync`
  returns false for a display-locked subtree (`resize_observation.cc`), which
  is web-core's `layoutchange` behaviour on `content-visibility: auto` list
  items. The update is gated by a stale bit that
  only a layout pass that ran, `observe_resize` and a target's free set
  (never a scroll or a render that laid nothing out), and it does nothing
  before the first layout. `notify_resize_observers` is the spec's broadcast
  with the intersection loop's re-lookup-by-id and handler-out discipline;
  the host runs it from a posted entry after the frame (ruling 2), so the
  spec's depth filter and loop error are not built. `disconnect` clears the
  queue, as the spec clears `[[activeTargets]]`; `take_resize_records` is
  an engine addition. A page without observers pays one `is_empty` per freed
  node and per update and one bool per layout pass.
- Stylo's per-element style data and its traversal/invalidation flags live
  inline on `Node` (bench-defended 2026-08-03: no traversal regression, a
  measurably faster no-op-commit fast path).
- One `StylePool` per document through `Document::set_style_pool`;
  `MAX_STYLE_THREADS` is six, a ceiling rather than a tuning knob.
- Confirm the vendored fork tip with `git -C vendor/stylo rev-parse --short
  HEAD` rather than trusting a written one.

Everything else about the crate's internals is in `docs/terminus-architecture.md`.
