# `crates/dom` architecture

`crates/dom` is the generic W3C-DOM-subset document tree and
standards-oriented CSS computation core the Lynx runtime layers sit on.
`docs/dom-public-api.md` is the authoritative normal-build versus test-feature
API boundary; this file is the internals, moved out of `AGENTS.md`, whose
`crates/dom` section keeps the charter and the rulings an agent needs before
touching the crate.

## Tree, arenas and identity

It owns a fixed-address boxed `TreeArenas<T>` containing two `Slab`s: a primary
`Slab<Node<T>>` (slot zero is the real DOM Document node and carries its
node-visible style context; later slots are element/text nodes) plus a
slot-aligned payload slab. A separate inline `DocumentLayoutState` owns
slot-aligned layout state — a lazily sized vector rather than a third lockstep
slab, so a node that is never laid out allocates none,
`Document::layout`/`render` size it to the arena's slot bound in one resize,
and an absent entry reads as "never laid out" (an empty layout cache).

Identity and storage are deliberately split (`tree/arena.rs`): a `NodeId`
indexes `TreeArenas::slots`, a `Vec<Option<NodeSlot>>` holding the arena slot
the node's state occupies. Ids only count upward and a freed one is never
reissued, so a `NodeId` names one node for the document's life and a stale id
resolves to nothing rather than to a stranger — hence no generation counters,
no epoch gate on the retained frame, and the same number handed to script as
Lynx's element `unique_id`. The arena slot *is* recycled; only the four-byte
table entry leaks, per node ever created. The raw slabs are private to
`tree/arena.rs`, so nothing else can index one with a `NodeId`. Stylo's
per-element style data (the upstream `ElementDataWrapper`, no outer cell) and
its traversal/invalidation flags live inline on `Node` (bench-defended
2026-08-03: no traversal regression, a measurably faster no-op-commit fast
path). The primary slab selects each raw-`usize` ID; the payload slab allocates
and removes in lockstep and asserts the same key (reserving a payload-less
sentinel at document slot zero), while the layout vector resets a freed key's
entry so the next occupant starts clean (ONE TREE policy: nodes are created and
mutated only through `Document` methods). Two more slot-keyed side tables live
on `TreeArenas` itself rather than beside it — `content-visibility: auto`
relevance (`layout/relevance.rs`) and each element's last committed content box
(`layout/committed_box.rs`), which carries both the css-sizing-4 last
remembered size and the css-contain-3 query container size — because a
`StyleView` is built from these arenas alone and the layout state is a
separately borrowed parameter it cannot reach. Both are lazily sized, so a page
that uses none of those features allocates neither, and both reset on free like
the layout vector does. The committed-box table is the one the layout pass
*writes*, and `LayoutTree::compute_layout` holds the arenas shared, so its
records are staged through a `RefCell` and published by `Document::layout`
under the exclusive borrow, once per pass — the published half has to be a
plain `Vec`, because the parallel style traversal reads the container half. The **document element is permanent
and pre-created**: `Document::new(device, root_tag, root_payload)` builds it at
slot one (tag injected — the core owns no tag vocabulary), `document_element()`
returns it non-optionally, and it can never be detached or removed, so the
document node's child list is structurally immutable after construction and no
"empty document" path exists in flush, layout, visual or paint. Computed styles
remain with the primary nodes; layout/text state does not.

The crate's entire `unsafe` surface is two blocks — the arena backpointer deref
and the `TElement::ensure_data` contract call — plus the `unsafe fn` signatures
Stylo's traits mandate (all bodies safe). Both blocks carry a `SAFETY` comment
stating the invariant they rest on, and a crate-local
`#![warn(clippy::undocumented_unsafe_blocks)]` keeps that true.

The generic `T` payload remains associated with each element/text node in the
NodeId-aligned payload slab but is opaque and read-only to the DOM core;
selector-visible state comes only from real DOM fields, so payloads cannot
synthesize attributes. DOM setters own snapshot/restyle scheduling, while
stylesheet and device methods on the document schedule its root in the same
call — embedders cannot set/clear dirty state or write computed styles.
Mutation APIs follow a let-it-crash contract (`debug_assert` + panic on stale
handles rather than silent no-ops).

The embedder-facing `dom::Device` profile exposes exactly the inputs that vary
between views — `Device::new(width, height, device_pixel_ratio)` — and locks
the rest: screen media type, standards (no-quirks) mode, light color scheme,
coarse touch pointers, CSS-values-4 fallback font metrics. Quirks stays
hard-wired in matching, the `Stylist` and the doc-hidden `standards_device`
test seam, so neither the quirks knob nor any stylo device vocabulary exists
above this crate; view metrics read back through `Document::{viewport_size,
device_pixel_ratio}`. The crate root re-exports the one workspace `vello`
version — embedders configure wgpu/peniko/kurbo exclusively through it — and
re-exports `stylo` as the CSS vocabulary door for the layers above (strict
linear chain: cli → resources → core → element → dom).

It must not contain Lynx runtime-element vocabulary or Lynx device/unit policy
— Lynx computed defaults (border-box, `overflow: clip`, `display: linear` on
every element, …) stay embedder cascade policy (UA sheet).

## Shadow DOM and custom elements

**Shadow DOM** (W3C, so W3C behavior) adds a fourth `NodeData` kind:
`Document::attach_shadow(host, mode)` creates a shadow root attached to its
host rather than listed among its children, so a host's child list stays its
light children. Three trees then coexist. The **node tree** is what selectors
match and what public `Node` navigation reports; a combinator runs out of
parents at a shadow root and Stylo retries against the featureless host, which
is what makes `:host` — and only `:host` — reach across. The **flat tree**
(hosts replaced by their shadow trees, `<slot>`s by their assigned nodes or the
slot's own children as fallback) is what Stylo traverses, what inherited values
inherit through, and what layout, paint and hit testing walk; it is reached
exclusively through `Node::flat_children`/`flat_parent_id`, both returning
arena slices so every consumer keeps its `&[NodeId]` iteration. Each shadow
root owns an `AuthorStyles<DocumentStyleSheet>` whose scoped `CascadeData`
(`Document::add_shadow_stylesheet`) replaces the document's author rules inside
that tree; `::slotted()` and `::part()`/`exportparts` work off the same data.
Slot assignment is eager — every mutation that can change it (host child list,
shadow-tree slot set, `slot`/`name` attribute) resolves the affected tree in
the same call, gated on a live-shadow-root counter so a document with none pays
one branch — but appending or removing a light child touches only the slot
involved (the shadow root caches its slot list, rebuilt only when the slot set
changes and debug-checked on every hit), and full reassignment is reserved for
cases that can re-target more than one node. That split is benchmark-defended:
with the append path reassigning the whole tree, building a 1024-row host cost
51× the same rows with no shadow root, and 1.4× after
(`benches/shadow.rs::build_wide_host_{plain,shadow}`, paired
plain-versus-shadow throughout). Per-node cost is one
`Option<Box<ShadowLinks>>` word, allocated only for hosts, slots and slotted
nodes; the flat tree costs nothing on a no-op commit and ~1.02× on a frame.
Recorded limits: `TElement::slotted_nodes` keeps Stylo's empty default
(assignment changes dirty the host subtree wholesale instead of invalidating
`::slotted` per slot), `:host-context()` is absent from the vendored selector
grammar, and a node that leaves the flat tree keeps its last computed style and
geometry — the contract detached subtrees already have.

**Custom elements** (W3C behavior within a deliberately narrowed scope) are the
other half of the component model. `Document::define(local_name, Box<dyn
CustomElement<T>>)` registers one handler per tag — per-tag rather than the
standard's per-instance constructor, because this crate has no script realm to
hold instances in, so every callback names its element by `NodeId` and
per-element state belongs to the layer owning `T`. The handler receives
`constructed`, `connected_callback`, `disconnected_callback` and
`attribute_changed_callback`, the last filtered by an `observed_attributes`
list read once at definition time, plus `handle_event` — not a lifecycle
callback but the engine-side event hook, which is how a component hears
something the engine decided about it or about its descendants (see the
`event` module above). **Scope: user-agent components, not
script-defined elements.** Definitions come from the engine layer above, never
from application script, and `define` *requires* that every definition precede
any element with its tag — it panics otherwise. That contract removes the
standard's entire upgrade half: no `undefined` state and therefore no
`:defined` transition, no *upgrade an element*, no *try to upgrade*, no
`define`-time document sweep, no replay of attributes an element already
carried, and no *valid custom element name* predicate. The document element is
the one exception, because `Document::new` creates it before any definition can
exist, so defining its tag constructs it. Restoring script-defined elements
later is additive — an `undefined` state, an upgrade reaction, a sweep — and
moves neither the trait nor the dispatch contract. What the narrowing does
**not** remove: reactions are still **queued, never called inline**, and
drained at the end of the public mutation that raised them (the standard's
`[CEReactions]` boundary), because a lifecycle callback mutates the tree while
its handler lives inside the `Document` being mutated. Dispatch clones an
`Arc<dyn CustomElement<T>>` out of the registry rather than vacating the slot,
so a callback on `x-row` can create another `x-row` instead of hitting a
re-entrancy panic. Scopes are watermarks into one flattened element queue while
the per-element reaction queue is shared across them, reproducing a browser's
`A.disc, A.conn, B.disc, B.conn` for a subtree move. Three `Node` fields carry
the definition pointer, the `Uncustomized`/`Constructing`/`Custom` state and a
conservative shadow-including-subtree summary; all fit in the existing tail
padding (stride unchanged, asserted). The summary rejects a lifecycle walk at
an ordinary subtree root and prunes ordinary branches when a walk is needed;
insertion propagates it upward, removal may leave harmless false positives.
Reaction scratch collects only constructed custom elements; `Constructing`
earns its byte by suppressing the reactions a constructor's own mutations would
raise back at it. `:defined` is answered but never moves — with no `undefined`
state it matches everything, which is why the `:not(:defined)` FOUC idiom is a
script-defined-elements feature. A nesting depth and a per-scope fixpoint
budget bound the drain, and both panic rather than hang. This is the crate's
first self-authored `dyn` (the other two are mandated by upstream Stylo
signatures), admitted by explicit user ruling because a document holds N
behaviors keyed by N tag names discovered at runtime, which a type parameter
cannot express. The trait carries no `Send + Sync` supertrait: a document is
deliberately not `Send`. Benchmarked (`benches/custom_elements.rs`, three-way
plain/unmatched/defined): a document that defines nothing pays 1.00× on a no-op
commit and 1.01× on creation, and the suite's 4096-descendant `remove_element`
cases defend the unmatched-definition negative fast path and the dense callback
path separately. Further recorded limits: no `adoptedCallback` (no second
document exists), no `connectedMoveCallback` (every move is
disconnect-then-connect, the standard's own fallback), no customized
built-ins/`is`/`extends`, no scoped registries, no
`whenDefined`/`get`/`upgrade(root)`, and no `failed` state or construction
stack — all of which polices a JavaScript constructor that can throw.
`disconnected_callback` takes a shared `&Document`, not a mutable one: it is
the only callback that runs with a free already committed, so a mutable handle
would let it re-attach the subtree being freed, link a child to a node about to
die, or free the node its caller still holds. A callback that *can* mutate may
detach any node but may not *free* one the calling mutation still holds:
`create_element` and the constructor call pin that id, and
`drop_element`/`drop_subtree` refuse to free a pinned node, since freeing
retires the id permanently.

## Style engine and invalidation

Every node points directly back only to `TreeArenas`, and the same plain
one-word `&Node` implements Stylo's document/node/element/shadow-root traits
according to its `NodeData` (styling runs in place, no mirror tree),
inline-style parsing, and a private per-document `StyleEngine` containing the
`Stylist`, cascade pipeline, device, stylesheet set, and `SharedRwLock`.
`Document::new` creates that context afresh, so different documents cannot
share stylesheets. Author CSS enters either as text (`add_stylesheet`) or, for
CSS a host already parsed, as rules the document itself builds —
`build_style_rule` / `build_keyframes_rule` / `build_font_face_rule` mint an
opaque `CssRule` branded with the lock that created it, and `append_rules`
mounts a batch of them as one sheet, refusing any rule minted by another
document. That keeps the `SharedRwLock`, the base URL, and stylo's own rule
types inside the crate while letting the layer above skip the sheet, at-rule,
and declaration-block parsers.

A document's style traversal runs on the workers `Document::set_style_pool`
gives it and on no others: one `StylePool` per document, moved in rather than
shared, which lets two documents restyle at the same time with nothing
serializing them. A document that was never given one — every test and
benchmark here, and any embedder asking for a sequential view — traverses on
the thread that flushed it, so the CSS benchmarks measure cascade and matching
work rather than Rayon dispatch.

Style flush and its per-node `StyleDamage` (repaint / stacking / overflow /
relayout classes) are internal parts of `Document::layout`; harvested damage is
then **cleared** (the fix for stylo's never-cleared-damage re-traversal bug).
During that same harvest, relayout-class damage is consumed immediately into
boundary-stopped layout cache invalidation, so no external damage report is
needed to preserve layout work; the module also owns the
`effective_containment` fold (`contain` + `content-visibility` → effect bits).
Layout invalidation stops early at the deepest ancestor whose committed input
hughie marked **content-independent** — the committing parent proved the
input's known dimensions, parent size, and available space cannot move when
only that subtree's content changes (pure-length sizing, stable percentage
bases, imposed stretch, content-free automatic minimums, chained from the
viewport-anchored root input; distinct from CSS *definiteness*, which admits
content-measured sizes). `run_layout` relays such a subtree in place under the
stored input and accepts the result only when the output reproduces bit for bit
— anything else escalates to the whole-tree pass, which reuses the caches the
attempt just filled. This is what makes the ReactLynx steady state
(text/attribute updates inside fixed-size rows under the
`page { width/height: 100% }` UA anchor) cost a subtree instead of the
document; `contain: strict` boundaries keep their parked-relayout path as the
containment-guaranteed special case of the same machinery. Second and later
invalidations in a batch stop at the first already-cleared ancestor, so a burst
of mutations pays one spine walk. Equivalence tests
(`tests/incremental_relayout.rs`) pin every path — in-place, escalated, and
root-reaching — to the geometry of a fresh document built directly in the final
state.

## Layout host

Its `layout` module is the concrete `hughie` host: `Document::layout` flushes
styles then lays out with the single `LayoutTree` trait implemented on
`TreeArenas<T>`. Plain `NodeId`s identify nodes, and every engine entry
receives `&TreeArenas` alongside a separate `&mut DocumentLayoutState`; there
is no `LayoutTreeView`, session, or store adapter. After each completed
traversal, the exclusive damage harvest clones every visited element's primary
`Arc<ComputedValues>` into a per-node layout-style snapshot; layout/paint
borrow that snapshot with no `ElementData` borrow check or per-read `Arc` bump,
and the `Arc` keeps the value alive, so reads are always memory-safe. The
harvest descends wherever Stylo's dirty-descendants bits point *or* the
element's own snapshot identity changed — the latter covers initially styled
and freshly cleared (`display: none`) subtrees, which set no dirty bits. A
debug assertion at every snapshot read reports divergence from Stylo's live
primary style; release builds read the stale-but-owned snapshot instead of
crashing. Public computed-style access still uses Stylo's guarded borrow.
Layout and text state use ordinary exclusive Rust borrows with no runtime
borrow checking. Display dispatch routes flex/grid/linear/relative with
`display: none` hiding and a leaf fallback, text nodes through concrete Parley
measurement, and the positioned pass implements the W3C `position: fixed`
containing-block rule via the protocol's scheme override. `display: contents`
elements generate no box: the engine's `flattened_children` splices them out of
every item collection, and the host denies them containing-block, containment,
skipped-contents, and hoisting status and zeroes their `LayoutSlot` in the
positioned pass (the document element is exempt — Stylo blockifies it).
Replaced leaf content reads a closed `NaturalSize` value stored in lazily
allocated node content; its internal update path automatically invalidates the
affected cache path. Mutually exclusive literal text, natural size, and
test-only leaf metadata reuse the node's single nullable content pointer.
`Document::set_natural_size` and `Document::set_image_source` are the public
replaced-content update seams (public because both halves arrive from above
`dom`, out of the embedder's image system, independently and in either order).
The natural size always invalidates layout; a source invalidates only the scene
*unless* it is the call that makes the element replaced, because being replaced
forces `DisplayMode::Leaf` and hides every child — a layout input, not a paint
one. Both getters stay paint/layout-internal, setting an equal value is a
structural no-op, clearing a source an element never had is a no-op rather than
a conversion to a replaced leaf, and the DOM core still knows no tag names.
Each `DocumentLayoutState` entry owns one `LayoutSlot` containing the
measurement cache, static position, and durable rounded/unrounded results;
`Document::rounded_layout` is the public geometry query, while unrounded
geometry and cache contents stay internal (the cache probe is `#[cfg(test)]`).
`Layout` is non-`Clone`; rounding reads its `Copy` fields and constructs the
rounded record without duplicating the whole value. Style-driven relayout is
automatic (every style flush consumes harvested `StyleDamage` into
boundary-stopped invalidation); the internal invalidation funnel covers
mutations styles cannot see (content/child-list changes with identical computed
styles). Public mutation methods perform that invalidation themselves; only the
`layout-test-utils` feature exposes an explicit benchmark hook.

## Visual order, paint and the committed frame

Its `visual` module owns the post-layout visual order: the full W3C
stacking-context predicate, CSS2 Appendix E paint order (a private flat
back-to-front `PaintOrder` of items with viewport-space transform matrices and
overflow/`contain: paint` clip chains that honor containing-block escape),
transform resolution (transform + transform-origin + parent perspective, always
flattened — the fork has no authorable `preserve-3d`), and reverse-paint-order
hit testing (`Document::elements_from_point{,s}` and input targeting, pure
reads of the frame the last render retained, honoring `visibility`,
`pointer-events`, border-radius, and inverse-matrix point mapping). It walks
the same flattened box-tree the layout host feeds the engine, so
`display: contents` dissolves identically in paint and hit order. Group-effect
stacking contexts (`opacity`, `filter`, `clip-path`, `mask`, plus the
storage-only blend/isolation triggers) additionally surface as `RenderLayer`
entries — preorder, parent-linked, each with the establishing element, its
world transform/size, and the contiguous item range the group encloses — which
is what the document-owned Painter composites; group effects still do not
affect hit testing (recorded limit). Lynx-specific hit-test policy (hit-slop,
`user-interaction-enabled`, event-through) belongs to the future runtime-policy
layer, never here.

**`content-visibility: auto` relevance is decided here**, not in layout and not
by a host. `render` claims one commit id, builds the paint order, and asks
`visual/relevance.rs` whether each `auto` element's own border box still
reaches the region *that frame's culling* admits — the walker's own
`CullPlan::admits_box`, which `plan_frame`'s first cull test also calls, over
the same resolved clip chains, `ScrollSlot::encode_window`s and group blur
reaches, so "relevant wherever its contents could paint" holds by
construction. The build records an `AutoBox` (node, world transform, size,
clip, compose space, enclosing group layer) for every `auto` element it
reaches, independently of the item list: a `visibility: hidden` element emits
no item and still has to be determined, or its `visibility: visible` contents
would never lay out. A flip invalidates layout through the ordinary relayout
machinery and the frame is rebuilt under the same commit id, at most four
times, with each element determined at most once per commit; the published
frame is always the last pass's, so no frame carries a pending reveal. The bit
itself is layout-side per-element state in a slot-keyed side table on
`TreeArenas` (`layout/relevance.rs`) — never a Stylo `ElementState` and never a
restyle trigger — read through `StyleView`'s `CoreStyle::skips_contents`
override, which is the single answer the layout host, the relayout
invalidation walk, the paint-order build and the stacking predicate all take.
A page with no `auto` element pays one `is_empty` test per render. Its
counterpart is the **last remembered size** (`layout/committed_box.rs`), which
decides what a box that skips is sized *from*: the layout host records a box's
content box after every committing run in which it had no size containment,
and `StyleView` substitutes it into `contain-intrinsic-*` once the box starts
skipping, so a revealed row that goes back off screen keeps the height it
rendered at instead of collapsing to its estimate.

`Document<T>` owns one private concrete `Painter`
(`crates/dom/src/paint/painter.rs`), including its reusable walk scratch, its
retained `vello::Scene`, and the `Arc<CommittedFrame>` it retains. `render`
privately builds `PaintOrder` and invokes that painter only for a dirty scene;
it records which private visual epoch its scene represents, so
`render`/`needs_render` own retained-scene scheduling without publishing that
epoch. `commit` publishes the same retained `Arc`, whose tables answer hit
tests and scroll-chain walks with no document; `committed_frame` hands it back,
and `scene` lends a guarded shared borrow. There is no renderer type parameter,
`DocumentRenderer` trait, `with_renderer`, public Painter, public visual epoch,
or public paint-order constructor.

**The frame is baked unscrolled** and carried split — per-space scene fragments
plus a compose program — so a consumer composes at its own current offsets per
`ScrollSlot` and a scroll recomposes instead of recommitting, for as long as
every offset stays inside its slot's `encode_window`. A slot's
`viewport_axes` maps its local scroll offsets and encode window through the
container's transform, so a scaled or rotated scrollport moves content in the
same coordinate system as its sticky offsets. When one leaves the window,
`note_scroll_windows_stale` is the consumer's refill request, which the painter
sends as `ToMain::Refill { offsets }` and the main thread answers with a
recentered commit. `Document::scroll_to` applies the same rule to its own
writes: a scroll the retained frame's slot can still compose invalidates
nothing, and one past that slot's `encode_window`
(`CommittedFrame::covers_scroll_offset`) makes the frame stale, because past
it there is no encoded content to compose and no `auto` box was determined
for it. Composition is the one render path: `compose_into` replays
the whole program into one flat scene at those offsets, and nothing is retained
per scroller. A scroll
container is no stacking context by itself, as on the web (see
`runtime-architecture.md`). Composite
animations ride the same split: an exportable `opacity`/`transform` animation
publishes an `AnimationSlot` curve the consumer samples at its own timeline
reading. What moves at composition is recorded as one compose space tree
(`visual/space.rs`): scroll, sticky and animation nodes in containing-block
order, each applying one affine, with every fragment, push, image draw, item,
clip and filter entry naming its innermost node. An element's own box, clip and
effect layer take its *box space* — inside its own sticky and animation nodes,
outside its own scroll node — and its content the *content space* inside that
scroll node. A record's map is the product of its path's node affines, root
first, formed in one place (`SpaceSamples::css`) that composition, filter bakes
and hit testing (inverting it) all read, so a scroll container, a sticky box or
a clip inside an animated subtree moves exactly as a fresh commit would place
it. `docs/dom-public-api.md`'s "Retained visual output" row is the
authoritative description of the whole surface.

**Sticky positioning also resolves in that compose path.** A private constraint
table records each sticky box's normal geometry, physical insets, containing
block, and nearest scrollport per axis. Grid items retain their grid area as
the containing block; a direct child of a scroller can move through its scroll
content. Sticky offsets are evaluated from the consumer's live scroll offsets,
shared by painting, clipping, hit testing, and the document's bounding-rectangle
query. Descendants inherit the motion through their containing-block chain,
so viewport-fixed descendants still escape it. Culling preserves possible
sticky travel across the frame's encode window, including a header whose
normal-flow position has scrolled out of view. A sticky box inside or around
a compositor-exported animation keeps it exported. Its constraints are solved
in layout space from scroll offsets, which no transform changes; the solved
shift is mapped through the committed parent transform and applied by the
box's own sticky node at its place on the space path, so an enclosing
animation's delta maps the shifted box, and the shift carries an animated
descendant along with its delta. Inline sticky atoms inside a paragraph are
not pinned: the paragraph paints them, and the sampling covers only boxes with
items of their own.

`filter: blur()` and `backdrop-filter` add the one conditional step in front of
that path, and they share it. `CommittedFrame::filter_groups()` — empty unless
the page uses one of them — carries per entry a device-pixel σ and rect plus a
range of program ops, and `FilterGroup::is_backdrop` is which of the two an
entry is.

A **`filter: blur()` group**'s rect is already 3σ larger than the group's
content, so the bake's clamped edges read transparent black, and its range is
the ops the entry encloses, bracketed by `PushFilter`/`PopFilter`. A
**`backdrop-filter` entry**'s rect is exactly the element's transformed border
box — the crop filter-effects-2 applies before filtering, with no margin
because the property enlarges no ink overflow — and its range points
*backwards*: every op from its nearest Backdrop Root ancestor's content start
up to the element's own scope open, which is exactly "everything painted before
the element inside that root". Its one op, `PushBackdrop`, is recorded
innermost inside the element's own layers and before any of its items, so the
element's `opacity`, `clip-path`, `mask-image` and `filter` apply to the
backdrop and to the element together.

Neither op encodes anything without a texture, so one program serves both
readings: `compose_into` takes a `filtered` table and, where one exists, draws
a group's texture in place of its ops or a backdrop's through the element's own
rounded border box; where none does, a group replays its range raw and a
backdrop draws nothing, which are the documented *unfiltered* fallbacks a
GPU-less consumer, `Document::scene()`, and an entry past the memory budget all
take. `CommittedFrame::bake_filter` replays one entry's range into an offscreen
scene with the entry's own space divided out, since that space is applied when
the texture is drawn; for a backdrop it then pops the layers the backward range
left open and draws the list's pre-blur passes over the whole bake rect. The
whole commit stays device-free and recyclable: the entry table is reclaimed
with the frame's other tables.

Because a `filter: blur()`'s 3σ ink margin has to survive the viewport edge,
the walker inflates a filtered layer's bounds *before* intersecting them with
the viewport, and inflates the cull region by the sum of 3σ over every
enclosing filtered layer. `backdrop-filter` inflates nothing. Either way σ is
scaled into viewport pixels by the arithmetic mean of the two singular values
of the element's local-to-viewport linear map (read off
`Affine::nuclear_norm_squared`) — exact under rotation and uniform scale, one
isotropic number under a non-uniform scale or a skew (recorded limit).

An ancestor's `overflow` clip cuts a filter's output, not its input. A scope
whose element has `filter` or `backdrop-filter` opens inside the links of its
own clip chain — the output clips, pushed outside its layers, the innermost
as a full `SrcOver` layer for the #1198 rule — and its content pushes only the
links below them; every other scope re-pushes whole chains so a fixed
descendant of an `opacity` group escapes the group's ancestors' clips. Culling
follows the same split: a link a blurred group's own chain holds cuts after
that blur, so the cull test grows it by the 3σ of every blur applied before
it, and a group's bounds hold moving content to its chain grown by the blurs
*around* the group (the group's own margin is added when it closes).

The Backdrop Root set is filter-effects-2's list — `filter`, `opacity < 1`,
`mask`, `clip-path`, `mix-blend-mode`, `backdrop-filter`, and the root element —
plus an element with a current `opacity` animation, exported or not, at every
reading: Web Animations makes it act as `will-change: opacity`, and a
descendant's backdrop range fixed at commit is composed at every instant of an
exported curve. It is a separate predicate (`walker::is_backdrop_root`) rather
than `stacking::needs_group_rendering` because that one also answers `true` for
`isolation: isolate`, which the spec's list does not contain. `will-change`
roots are **not** honored, which is the one observable gap: `will-change` is in
the fork's author grammar, so a `backdrop-filter` element inside a
`will-change: opacity` wrapper reads through that wrapper where a browser would
stop at it. Ruled, and recorded in `docs/tracking/deviations.md`.

The image seam is the `FrameImages` trait in `crates/dom/src/render/image.rs` —
`read(source, ImageSizeHint)` and `retain(frame)` — with `ImageReports`,
`ImageInbox`, `ImageEvent` and the `NoImages` no-op store beside it. The
document requests sources through `take_wanted_images`, receives answers
through `apply_image_events`, and retains no pixel: the store is the
consumer's, handed to `scene` and to the bake path per call.

The private `painter`/`walker`/`paint`/`shape` modules turn the paint order
into the retained Vello scene. Item clip chains diff against Vello layers;
`RenderLayer` scopes composite opacity, filters, clip paths, and masks; box
fragments paint shadows, backgrounds, replaced content, borders, outlines, and
retained Parley glyphs. Internal style access is `Document::paint_style`
(post-flush, no `Arc` bump), geometry is the rounded layout, and the document
Device supplies viewport/DPR so paint cannot disagree with layout. The
authoritative paint limits are recorded in `crates/dom/src/paint/painter.rs`;
DOM-aware paint tests and the paint benchmark live under `crates/dom/tests` and
`crates/dom/benches`.

The crate also owns the DOM-free render floor absorbed from the former `pulsar`
crate (2026-08-04): the `render` module holds the `FrameImages` trait
(re-exported at the crate root) and the `render::gpu` wgpu
render-to-texture/readback backend (`gpu::Headless`, plus the
`read_texture`/`renderer_options`/`render_params`/`AtlasResidency` seams
windowed embedders build against). `render::blur` is the third seam, and the
one narrow exception to the floor's DOM-freedom: `FilterTextures::prepare`
reads a `CommittedFrame`'s filter side table and calls the frame's own
`bake_filter`, because a bake *is* a partial replay of that frame's compose
program. It binds one of two samplers per entry — clamp-to-edge for a
`filter: blur()` group's transparent margin, `MirrorRepeat` for a backdrop's
marginless crop — and skips the whole gaussian at σ = 0, which only a
colour-only `backdrop-filter` reaches. It still knows nothing about nodes, computed styles, layout or paint
order — the frame hands it device-pixel geometry and an opaque op range — and
nothing else in `render` names a frame at all. A render names the bitmaps its scene
draws: vello frees its persistent image atlas whenever a scene with no patch at
all renders while its image cache still counts every resident image clean, so
`AtlasResidency` — one per `vello::Renderer` — re-marks each image once after
such a loss and nothing in the steady state. `Headless::new` reports `NoAdapter`; every GPU-backed test
treats that as a hard failure, including in CI.

## Scroll, input and event paths

Its `scroll` module owns CSSOM-View scrolling — scrollport/scrolling-area
geometry off the layout engine's accumulated `content_size`, a per-node offset
in the layout arena that re-clamps itself on every read (so a shrinking
relayout or a restyle out of scroll-container-hood needs no invalidation hook),
`scroll_to`/`scroll_by` (which returns the **unconsumed remainder**, the
primitive chaining is built from), and `scroll_chain`. Both the "which box
scrolls" walk and the chaining advance follow the **containing-block** chain,
not DOM ancestry, so they agree with what `visual` actually moves: a wheel over
an `absolute` box anchored above a scroller scrolls nothing. The chain walk
itself is one function, `drive_chain` over a list of `ChainLink`s, that the
document runs over live geometry and `bobcat-core`'s painter runs over the
committed frame's scroll-slot table, so the two agree on order and reach:
`overscroll-behavior: contain | none` fences everything above a container on
that axis, and the engine's own `scroll-capture: nearest` (no W3C or Lynx
counterpart) visits the container above first and this one only once that
ancestor cannot move. `scroll/snap.rs` is css-scroll-snap-1 without its
events: a container's snap positions are computed from layout and published
beside its slot, a drag settles onto the nearest one at release, a wheel tick
steps to the next one in its direction, and a snapping container is re-snapped
at rest whenever a commit lands. Only
`overflow: scroll` is user-scrollable; `hidden` is a scroll container that
moves only programmatically (load-bearing here, because the Lynx UA cascade
puts `hidden` on every element) and `clip` is not a scroll container at all —
it clips, has no offset, and its content does not reach into an ancestor's
scrolling area either (`hughie`'s `accumulate_scrollable_overflow` asks per
axis). `visual` bakes the offsets into the frame — a scroll container's
contents are translated as they are collected, with containing-block-keyed
escape sharing the clip chain's own struct — so painting and hit testing see
scrolled geometry and the lower render/GPU floor needs no knowledge of
scrolling. Clipping is likewise per axis, because `clip` on one axis with
`visible` on the other is a pair the style adjuster leaves mixed; a one-axis
clip is an infinite strip and carries no radii.

Its `input` module is the host seam: `InputEvent` is plain `Copy` data (pointer
+ wheel, viewport CSS px) that a canvas, a native window, or a test literal all
produce equally, and `Document::route_input(InputEvent)` is a pure read that
reports the node the event hit through the rendered frame. The crate has **no
default-action machinery and no recognizer**: deciding and driving the
user-agent scroll belongs to `bobcat-core`'s input router (`gesture.rs`), which
calls `scroll_by`/`scroll_chain` — whose unconsumed remainders exist for
exactly that caller. `InputEvent::default_prevented` is the `preventDefault()`
seam an embedder hands to that router after its own arbitration; this crate
never reads it.

Its `event` module is the other half, and it does **not** dispatch to script:
`Document::event_steps(target, bubbles, composed)` returns the ordered node
visits one event resolves to — the capture pass root-inward, the bubble pass
target-outward, the target in both — as plain `Copy` `EventStep`s owning no
borrow. Path construction is the standard's, including its shadow rules: a
slotted node's event parent is its assigned slot, a shadow root's is its host,
`composed` gates the crossing, and crossing retargets so every step from the
host outward reports the host. The shadow-crossing test is a single comparison
rather than the standard's per-step ancestor walk, and the equivalence is
argued in `event_path`'s doc comment and pinned by a differential test.
Dispatch itself belongs to `bobcat-core`, split across its two threads because
the realm cannot move and scrolling must stay responsive: the painter routes
the input and decides the user-agent scroll, and the script thread builds the
path from the exclusively owned document and delivers it there — that order is
what lets a listener mutate the tree. Nothing guards the window in between: a
`NodeId` names one node for the life of the document, so a step that outlived
its node resolves to no handle and reaches no one. There is no `preventDefault`
and no cancelable event anywhere on this path — Lynx dispatches none — so
suppressing a user-agent default action stays gesture arbitration's job,
arriving on the separate `InputEvent::default_prevented` seam.

There is a second dispatch, and this crate runs all of it: an event the
*engine* decides, delivered to the engine's own components rather than to
script. `Document::dispatch_element_event(target, kind, bubbles, composed)`
builds the same path and walks it here — capture pass root-inward, the target
once, bubble pass target-outward — calling `CustomElement::handle_event` on
every step whose node is a constructed custom element, so a `<list>` can hear
what a commit decided about its rows. It exists because handing the path up so
the layer above can call back down per step would buy nothing: these handlers
are Rust, owned by this document, and already called from inside its
mutations. The at-target capture step is skipped rather than delivered twice
(a component has one hook, not a registration set per phase); every hook takes
`&mut Document`, so the path is collected up front and each step re-checks
liveness before the call; each call is its own `[CEReactions]` scope, so what a
handler's mutation raised is drained before the next step. `ElementEvent`
answers `kind`/`target`/`current_target`/`phase` and carries both stop
methods behind one flag, because a node's local name resolves to at most one
definition. `ElementEventKind` is the extension point and has one variant,
css-contain-2 §4.4's `ContentVisibilityAutoStateChange { skipped }`, which
`Document::dispatch_content_visibility_changes` fires from the queue the
relevance pass filled — `bubbles`, not composed, not cancelable, and never
reaching a realm. A document that defines nothing pays one `is_empty` check
per dispatch and builds no path.

## Text

`DocumentLayoutState` lazily boxes the shared Parley `TextContext`; each
`display: -lynx-text` element's layout-state entry lazily boxes the
probe/commit `TextBlockStore` holding the one paragraph its subtree flattens
into. A text node generates no box and retains nothing: it is content of the
block above it, and its run reads inherited font/text values from its innermost
element ancestor. Font registration takes the shared `FontBlob` resource
through `Engine` → `Document` → `TextContext`; an owned loader buffer moves
into Parley without copying its payload, while `FontBlob::copy_from_slice` is
the explicit copying fallback. Relayout damage on an element evicts its direct
text children's measurement caches and retained artifacts, because text nodes
have no Stylo damage record of their own. Parley is unconditional and there is
no arbitrary payload callback.

## The vendored stylo fork

`dom` relies on the vendored stylo fork (`vendor/stylo`, tracking the canonical
`lynx` branch, tip `450b82af1`). `contain` was already seeded in the fork's
lynx grammar; fork PR #9 (squash-merged into `lynx`) added `content-visibility`
/ `contain-intrinsic-size` under the `lynx` feature, pref-gated for stock servo
builds; fork PR #10 un-gated `background-clip: text` from gecko the same way
and seeded the `outline-*` rows (`outline-offset` deliberately omitted — Lynx
outlines are flush rings); fork PR #11 seeded `object-fit` / `object-position`,
which were already ungated in `longhands.toml` and compiled out only by absence
from the allowlist — replaced content needs them for the css-images-3
concrete-object-size rules; and fork PR #12 un-gated `overflow: scroll | clip`
and added `Overflow::is_user_scrollable`. All four were squash-merged into
`lynx`. The native engine's grammar really is `visible | hidden`, but the
**web** bundle this stack consumes uses the other two directly (`web-elements`'
own `scroll-view.css` authors `overflow-y: scroll` and `overflow-x: clip`), so
no bundle could otherwise express a scrollable box. **`auto` stays out** (user
decision, 2026-07-29): this engine paints no scrollbars, so `auto` would be
indistinguishable from `scroll` everywhere except `to_scrollable()`, where it
is the value a `visible` axis pairs into — that now pairs into `hidden`, a
recorded deviation (an axis that genuinely overflows is clipped rather than
draggable). The three non-`visible` values stay genuinely distinct: `scroll` is
user-scrollable, `hidden` is a scroll container that moves only
programmatically, `clip` is not a scroll container.

Six commits have landed on `lynx` since #12, and the tip above is the last of
them: fork PR #14 required `Send` of `FontMetricsProvider` implementations and
fork PR #25 reverted it, restoring upstream's `Debug + Sync`. `Send` bought
exactly one thing — the right to *move* a `Device`, and with it a `Document`,
to the thread that would run it — and nothing does that: what crosses to
`bobcat-main` is a view's *sources*, and the document is built where it will
live. `Sync` is the bound Stylo itself needs, since the parallel traversal
shares one `Device` across Rayon workers by reference. Fork PR #13 corrects
`ElementData` reference documentation, fork PR #21 moves the `display`
longhand's initial value from `inline` to `Display::initial()`, which under the
`lynx` feature is `flex`, and fork PR #27 adds `DisplayInside::LynxText` — the
block-level, **non**-item-container value naming one flattened Lynx paragraph.
It is the cascade's way of saying what Lynx says structurally
(`TextElement::OnNodeAdded` converts every added child; no author CSS can undo
it), so a `<text>`'s subtree is inline content rather than child boxes. The
variant carries `#[css(keyword = "-lynx-text")]` because the derived
`DisplayInside` `ToCss` would otherwise kebab-case the variant name and
silently drop the vendor prefix. Fork PR #30 moves `content` out of
`LYNX_INTERNAL_LONGHANDS` and into `lynx_properties.txt`, so the `lynx`
feature's public grammar carries the `content` longhand the `raw-text`
generated-text rule depends on. Read PR #21 against the paragraph above rather
than as a contradiction of it: the *initial* value is what an element computes
to with no declaration reaching it at all, while Lynx's `display: linear`
default is a UA-sheet declaration this embedder cascades. Confirm the tip with
`git -C vendor/stylo rev-parse --short HEAD` before trusting this line — the
gitlink moves and the prose does not.
