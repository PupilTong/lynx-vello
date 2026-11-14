# crates/seldon

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/seldon`. Paths in backticks are relative to the repository root.

The Flexbox, Grid, grid-lanes, and Starlight Relative and Linear engine:
trait-based host⇄engine integration with static dispatch only (no `dyn`),
one `LayoutTree`
protocol with a `Copy + Debug` `NodeId`, immutable topology/styles for the
flush, and a separately borrowed mutable host state of per-node `LayoutSlot`s.
That split permits recursive mutation without copying style/layout records and
without `RefCell`/`AtomicRefCell` checks. Style traits speak the stylo fork's
computed-value vocabulary directly (requiring the `stylo` workspace dep +
python3 for its build script; the old zero-dependency/standalone pillar is
retired), with host-side display dispatch. They are split by algorithm:
`CoreStyle` carries the box model, containment, the alignment accessors and
`order`, while `FlexboxStyle`, `GridStyle`, `LinearStyle` and `RelativeStyle`
each carry what only their own algorithm reads and are demanded at that
algorithm's entry point; `GridLanesStyle: GridStyle` is the one that extends
another algorithm's trait rather than `CoreStyle`, adding `fit_tolerance` and
the computed `font_size` that `fit-tolerance: normal`'s `1em` resolves
against. `TextContainerStyle` supplies a paragraph container's CSS
(`text-align`, `text-wrap-mode`, `word-break`, `text-indent`,
`text-overflow`); `LynxTextStyle: TextContainerStyle` adds the Lynx `<text>`
inputs read from non-inherited integer custom properties — `text_maxline` and
`text_maxlength` (defaulting to unlimited), `tail_color_convert` and
`is_inline_truncation`. `LayoutTree::flattened_children` is the
box-tree view every algorithm collects items through, flattening `display:
contents` subtrees. Leaf content is deliberately closed: replaced content uses
the `NaturalSize` value path, text the crate's concrete
`TextBlock::probe`/`commit` paragraph path; arbitrary host measurers are not
supported.

**Flexbox, Grid, grid lanes, Relative, and Linear implemented** — the shared
root/leaf/cache/positioned/rounding machinery, CSS Flexbox Level 1, numeric CSS
Grid Level 2 (excluding subgrid/named areas), CSS Grid Level 3
`display: grid-lanes` on that Grid machinery, id-constrained Starlight Relative
Layout Level 1, and Lynx's `display: linear` algorithm and `linear-*`
style/source protocol are live. `LayoutOutput` carries a box's first and
last baselines; flexbox and grid align items by either (`baseline`, and
`last baseline` by the user's ruling of 2026-10-16), Linear exports a first
baseline only and Relative none, and the fork's `lynx` grammar parses
`baseline`, `first baseline` and `last baseline` for the item and self
alignment properties (`docs/tracking/deviations.md`). Grid lanes is a **user-directed W3C extension
beyond Lynx parity** (2026-09-18) rather than a compat obligation — native
Lynx has no such `display` value and no `fit-tolerance` property — and it
excludes `inline-grid-lanes`, the orientation property and
`grid-auto-flow: normal`,
`dense` backfilling, intrinsic `repeat(auto-fill, auto)`, virtual-item
grouping, stacking-axis self-alignment, baseline alignment/sharing, subgrid and
fragmentation; do not add any of those without a user decision
(`docs/style-assumptions.md` §24). Text shaping, line breaking,
intrinsic/height-for-width measurement, baselines, and retained Parley layouts
are unconditional crate behavior, in three modules. `src/text/paragraph` is the
shared Parley core with no Lynx semantics: whitespace normalization and the
byte↔UTF-16 source map (`content.rs`, its white-space processing chosen per
consumer), the run-style vocabulary and its Parley translation (`style.rs`,
with each run's `BreakStyle`), shaping, breaking and line capture
(`shape.rs`), tab stops (`tab.rs`, following the `text-indent` keywords), atom
`vertical-align` in parley's vocabulary (`position.rs`), the probe/commit break
memo and the `Retained` trait both consumers run the protocol through
(`memo.rs`, whose `BlockMetrics` carries the first and last line boxes'
baselines), the per-cluster extent walk (`extent.rs`), the source-range and hit queries
both kinds answer through (`query.rs`), the read-only bridge between a
committed paragraph's layout bytes and its source units that an editor
addresses either kind through (`offsets.rs`, `OffsetMap`, from
`TextBlock::offset_map` and `InlineFlow::offset_map`), `LineGeometry` — where each laid-out
line sits, parley 0.13's own CSS 2.2 §10.8.1 line box — and `Paragraph`, which
owns one shaped natural layout and re-breaks it in place, starting every line
with its root inline box (`ParagraphRoot`: the strut, which is parley's style
0, the base direction, and whether content with no text and no box still
lays out one empty line of the strut, which `terminus` asks for an editing host's
paragraph: `BlockStyle`/`InlineFlowStyle::strut_line_when_empty`). The core
knows nothing of inline boxes; a
consumer that lays text of its own into the content has four generic hooks,
which the Lynx block does not use: markers in the item stream
(`InlineItem::Marker`) whose places after white space collapsing the
normalizer reports beside the block (`MarkerPlaces`), segments spliced into
the normalized text with no source unit and a style of their own
(`NormalizedBlock::splice`), its own line-break override
(`shape::BreakOverride`), and advance laid beside an atomic box. What the core
reads of these at every shaping and break is the flow's `Additions` type
parameter (`Flow<A = ()>`, `Paragraph<S, A = ()>`): the Lynx block's is `()`,
the inline formatting context's `inline::edge::EdgeAdditions`, so the flow a
Lynx `<text>` retains carries none of it.
`src/text/block` is the Lynx `<text>` layer on it: `BlockStyle` with the
`text-maxline`/`text-maxlength` limits, the truncation in `truncate.rs` (which
lays an `<inline-truncation>` subtree in at the clamp; the painter applies
`tail-color-convert`'s native semantics), the `layout` event's line reports,
hidden-box outcomes, and the Lynx range and hit answers; its strut is the
establishing element's own font (`BlockStyle::strut`) and its bidi paragraph
level stays first-strong. `src/text/inline` is the CSS block container's inline
formatting context (`InlineFlow`, CSS-only `InlineFlowStyle`, items
`InlineFlowItem`): text runs, non-atomic inline boxes with their edges and one
`InlineFragment` per line each, and atomic inline boxes placed by margin box
and `vertical-align`. `inline/edge.rs` lays an inline box's inline-axis
margin, border and padding into the core's text as edge clusters — a copy of
the next character and a default-ignorable mark, shaped at size 0 with the
edge as its advance, no source unit, and a line-break override that keeps them
glued to the content beside them, because parley's own atomic boxes would be
a soft wrap opportunity on both sides — and adds an edge next to an atom to
the atom's advance; `inline/boxes.rs` is the box table (`InlineEdges`, each
box's place and content area). It also has css-text-3 §4.1 white space in
full (`break-spaces` included, collapsing across inline box boundaries),
per-run `text-wrap-mode`/`word-break`/`overflow-wrap`, the `text-indent`
keywords counted in the intrinsic widths, the container's `direction` as the
bidi paragraph level, its own font as the strut of parley's line boxes, the
fit-content width for a definite available width, and the Lynx block's
source-range and hit queries.

**css-anchor-position-1** is in the absolute pass: `anchor()`/`anchor-size()`
resolution, the `position-area` grid, `anchor-center`, the css-position-3
§4 self-alignment of every absolutely positioned box (flexbox, grid and the
host-placed boxes; Lynx `linear`/`relative` keep starlight's inset-only
placement) and the §6.5 fallback loop (`compute/anchor.rs`,
`anchor_area.rs`, `anchor_fallback.rs`), against `LayoutTree` methods whose
defaults mean "no anchors" (`anchor_rect`, `position_option_style`,
`set_anchor_outcome`, …). A box that uses none of it pays two style
predicates, and a `position_option_count` call only when it lists
`position-try-fallbacks`; an anchored box's own run reads its base
style and gets every differing axis as a known dimension, so it stays
cacheable (`docs/style-assumptions.md` §28, `docs/layout-architecture.md`).

**CSS containment (css-contain-2)** is landed layout-side: the stylo
`Contain`/`ContainIntrinsicSize` accessors on `CoreStyle`, size-substitution +
layout-containment baseline suppression, the skipped-contents pair
`compute_skipped_contents_size` + `hide_skipped_contents` (whose "is this box
skipping?" input is `CoreStyle::skips_contents`, defaulted to
`content-visibility: hidden` and overridden by `terminus` to fold in `auto`
relevance; the size is cacheable and goes through `compute_cached_layout`, the
hide sweep answers to the box tree and runs on every committing call outside
that cache), and
the `invalidate` module (`is_relayout_boundary`, `invalidate_for_relayout`) —
the containment-bounded, damage-driven cache-invalidation host workflow
(single-axis / container queries out of scope). `LayoutGoal::Commit` carries
per-axis `content_independent` flags: input *stability* under subtree content
change, proven by the committing algorithm (flexbox, grid, grid lanes, linear,
relative and
the absolute pass all set them; the root input is viewport-stable by
construction; a measurement carries no such claim, which is why only a commit
has the field). They ride inside the committed cache entry, outside its key, so
`LayoutSlot::committed_input` hands a host the complete input it can relayout a
subtree in place under and verify by output comparison. The per-node
measurement cache has a 32-entry ceiling but inlines two slots, spilling to the
heap for nodes whose containers probe many constraint shapes. Read
`docs/layout-architecture.md` before touching it. It must not depend on other
workspace crates or own host tree/style storage, DOM/runtime types, resolved
device-unit policy, or paint order.

The runtime-layout integration — the `LayoutTree` host, display dispatch,
hoisted out-of-flow box registry (their containing blocks lay them out),
per-node cache storage, and the automatic
style-damage→layout-invalidation wiring (boundary-stopped and engine-internal,
not a runtime-adapter concern) — lives in `terminus`; generic W3C text style,
document context, and artifact storage live there too.
