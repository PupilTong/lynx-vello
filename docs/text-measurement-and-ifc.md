# Text measurement and the bounded inline formatting context

Two halves. The first records what the measurement path does now and why —
the retained-layout contract `crates/hughie/src/text/` implements and the
eviction contract `crates/dom` drives it with. The second records the bounded
Flow paragraph now layered on top: sibling runs plus the four atomic
`inline-*` layout modes. It also isolates what is still open: transparent
rich-text spans, per-run paint/hit identity, and truncation.

Companion documents: `docs/layout-architecture.md` (the box-layout engine this
hangs off), `docs/tracking/css-text.md` (per-property Lynx conformance),
`docs/text-rendering-research.md` (the Parley/vello selection study).

---

## 1. What a text layout owner retains

One shaped Parley layout per owner, in `TextLayoutStore`
(`crates/hughie/src/text/layout.rs`), reached through `NodeLayoutState.text`
and lazily allocated by `text_parts`. A standalone W3C text leaf is its own
owner. Inside a Flow box, that element owns one mixed paragraph and its source
text nodes keep zero box geometry.

Shaping is the expensive half and is invariant under line breaking. Of the ten
vectors Parley's `LayoutData` owns (`parley-0.11.0/src/layout/data.rs`), eight
— `styles`, `inline_boxes`, `fonts`, `coords`, `runs`, `items`, `clusters`,
`glyphs` — are shaping output; only `lines` and `line_items` are rebuilt by
`break_all_lines`, and `BreakLines::new` swaps those two buffers out and clears
them, so a warm rebreak allocates nothing
(`parley-0.11.0/src/layout/line_break.rs`, `BreakLines::new` and its `Drop`).

Measured on this Mac for a 2 200-byte, 100-line Ahem paragraph: `Layout::clone`
is 7 allocations / 68 268 bytes / ≈1.42 µs, while one `break_all_lines` is 0
allocations / ≈17.5 µs. For a 28-byte label the two are within 25 % of each
other. **Line breaking, not copying, is what the measurement path has to hold
down** — which is why the retained layout carries three memos and no second
copy.

### 1.1 The three pieces of state

- **`broken`** — the `BreakConstraint` (`max_advance`, resolved `text-indent`)
  the current line output reflects. Re-breaking at the same constraint is
  skipped. Sound because two consecutive `break_all_lines` with the same
  `max_advance` after the same `set_text_indent` produce bit-identical
  `LayoutData`: `BreakLines::new` zeroes `width`/`height` and clears the line
  vectors, `BreakerState::default()` resets the cursor, and `Drop` rewrites
  `width`/`full_width`/`height`/`layout_max_advance` unconditionally. Verified
  over 12 096 configurations (RTL, CJK, empty, explicit newlines, negative and
  hanging indent, all three `IndentOptions`, `None` plus widths 5..260) with
  zero mismatches.

  The indent belongs in the key, not just the width. `indent` resolves a
  percentage against `definite_inline_size(input)` (`None` ⇒ basis 0 for
  min/max-content) while `max_advance` comes from `line_break_width`, so a
  min-content probe and a definite probe can arrive at the same `max_advance`
  with different indents.

- **`measured`** — up to three already-answered constraints and what they
  reported. A hit answers without re-entering Parley *and without moving
  `broken`*. This is the memo that matters in practice: containers do not ask
  monotonically. A flex text child is asked max-content, then min-content, then
  its used width, then max-content again; a break-state memo alone re-breaks on
  every alternation. Three entries because that is the number of distinct
  constraints an algorithm here produces; the replacement policy is round-robin
  and its worst case is one extra break.

- **`committed`** — the constraint *and alignment* the last commit left. See §2.

Cost: 128 bytes over Parley's own `Layout`, three quarters of it the remembered
constraints, on a struct that already owns ten heap vectors. Pinned by
`the_artifact_slot_is_one_pointer_and_the_break_state_rides_behind_it`.

### 1.2 What that is worth

`one_pass_breaks_a_text_node_once_per_distinct_constraint`
(`crates/dom/src/layout/mod.rs`) pins the result on the canonical Lynx label —
an auto-sized `text` element in a definite-width flex row. Nine measurements
reach the text node in one pass. Two of them break lines; the other seven are
answered from the retained layout. Before the memos all nine broke, and the
first probe additionally deep-cloned the shaped layout into a second slot.

Nine, not three, because the box cache (`crates/hughie/src/cache.rs`) keys on
the available height — which a text node's answer does not depend on — and
because each enclosing flex level asks its own intrinsic pair. That
over-specification is a box-cache question, not a text one; the text-level
memo is keyed on exactly what text depends on, which is strictly less, so it
collapses the duplicates the box cache cannot. See §9.1.

### 1.3 One slot, not two

The old design kept a `probe` slot deep-cloned from `committed` so a transient
measurement could not disturb the committed line breaks. It had no sweep: a
commit served from the box cache never enters the measurer, so the clone stayed
allocated for the life of the node, and a single-axis `Measure` — which the
committed cache slot can never answer (`packed_inputs_match` lets a stored
`Commit` answer only `Commit` or `Measure(Both)`) — forced that clone on every
flex child.

Now probes re-break the same layout a commit uses. Nothing holds a duplicate of
Parley's shaped vectors, and there is no second slot to leak.

---

## 2. The committed resting state

Everything outside a layout pass — painting, and only painting; see §9.4 —
reads the committed layout. A probe that moves `broken` away from `committed`
owes a `restore_committed` before the pass ends, and re-aligning is part of it,
because Parley zeroes each line's alignment offset on every rebreak
(`finish_line`, `line_break.rs`).

The obligation is real, not theoretical. A text node is measured and never
committed whenever the box cache answers its `Commit` but not the `Measure`
that preceded it, and every parked incremental relayout manufactures exactly
that state: `invalidate_layout` clears the parked node's whole cache before
parking it, and `run_layout` then recomputes it with a `Commit` only, so the
node ends the pass with a commit-only cache. A container computed under a
`Measure` goal also returns before committing any child.

`a_probe_that_never_commits_is_handed_back_to_its_committed_break` drives that
sequence directly.

### 2.1 Where the restore runs

The tail of `host::run_layout`, after the round walk — `round_with` →
`pre_position` → `position_hoisted` → `compute_absolute_layout` can itself
enter the measurer with a `Commit`, so anything earlier is not airtight.

It is driven by a queue (`DocumentLayoutState::probed_text`) pushed at the one
site that constructs a `TextMeasurer`, so the cost is O(probed nodes) rather
than O(slots) — the incremental path exists to avoid whole-tree work and a
per-pass scan of every slot would undo part of it. Debug builds re-derive the
answer the expensive way and assert the queue caught everything, because a text
node missing from it paints its probe's line breaks with no other symptom.

A never-committed layout is not painted: `TextLayoutStore::committed` returns
`None` until a commit has produced one, which is what the two-slot design gave
for free and what a `position: fixed` subtree measured for its static position
but committed only later in the round walk depends on.

---

## 3. Eviction: what kills a shaped layout

Two caches, two different questions. The box cache is invalid the moment any
geometry around a node moves; the shaped layout dies only when the text, or the
style Parley shaped it from, changes. `clear_box_cache` and
`clear_layout_cache` (`crates/dom/src/tree/arena.rs`) are the two answers.

The style harvest picks between them per relayout-damaged element
(`Document::invalidate_text_children`). Text children *always* lose their box
cache — `invalidate_layout` walks up, so nothing else clears a text child's
committed measurement — and lose their shaped layout only when
`shaping_inputs_changed` says so.

### 3.1 The two levels

`shaping_inputs_changed` (`crates/dom/src/layout/style.rs`):

1. **Pointer comparison** on `get_font()` and `get_inherited_text()`. Stylo
   allocates a fresh style struct only when the cascade actually applies a
   declaration in it (`StyleStructRef::build` re-shares a borrowed struct as
   the same allocation), so a purely geometric restyle answers here for free.
   This is the same test Stylo's own generated damage functions use.
2. **Field comparison**, only when a pointer differs, narrowed to the inputs
   themselves. This is what earns level one its keep: `text-align`,
   `text-indent`, `color`, `text-shadow` and the `-webkit-text-stroke` pair
   share `InheritedText` with `letter-spacing`, `word-break`,
   `text-wrap-mode` and `white-space-collapse`, so a struct-granular answer
   alone would still re-shape a paragraph whose only change was its alignment.

`direction` is deliberately absent: Parley is never told a base direction (bidi
comes from the text), so `direction` only selects the alignment a commit
re-applies anyway.

The verdict has to be taken inside `Node::refresh_layout_style`, while the old
and new styles are both alive. The caller sees only the new one, and the old
structs can be freed — and their addresses reused — the moment the swap
returns.

`a_relayout_keeps_shaped_text_unless_the_shaping_inputs_moved` pins the
distinction property by property.

### 3.2 The fingerprint

Keeping an artifact across a restyle is exactly the optimisation that fails
silently: a missed eviction paints stale glyphs and nothing else goes wrong.
Debug builds therefore hash, on every measurement, precisely the content and
style values that reach Parley's shaper — the run texts, `preserve_newlines`,
each run's family/size/weight/style/letter-spacing/line-height/features/
variations, and the container's `white-space-collapse`/`word-break`/
`text-wrap-mode` — and assert the retained layout was shaped from the same
ones. `a_retained_layout_that_outlived_its_content_is_caught` trips it
deliberately.

The fingerprint is the oracle for §3.1's field list. If a shaping input is
added to `translate_run_style` and not to `shaping_inputs_changed`, this is
what says so.

### 3.3 A defect the fingerprint's premise found

`harvest_animation_damage` mirrors the style harvest but omitted the
text-children invalidation entirely, so an animated `font-size`,
`letter-spacing` or `line-height` on a `text` element never re-measured its
text child: the element restyled and re-laid-out while the child answered from
the box cache it filled at the first font size. `@keyframes grow { from
{ font-size: 16px } to { font-size: 32px } }` held the text at 16px for the
whole animation.

Fixed by routing both harvests through the same
`invalidate_text_children`. `an_animated_font_size_remeasures_the_text_it_scales`
(`crates/dom/tests/animation_driver.rs`) fails on the pre-fix code with
`80.0 != 120.0`.

---

## 4. A defect this branch deliberately reproduces

The commit path re-breaks an auto-sized paragraph at its own measured width so
alignment distributes the right leftover. Parley does this for itself when the
break was unconstrained — its line breaker rewrites `inline_max_coord` to the
laid-out width — so only a finite constraint wider than the content takes the
second pass.

Re-breaking at `Layout::width()` is not neutral. That width excludes hanging
trailing whitespace while the breaker's fit test includes it (`next_x = x +
advance; if next_x <= max_advance`), and it is produced by a subtraction while
the breaker accumulates forward. Over 25 385 shrink-eligible configurations the
second break changed the line count in 4 303 of them, always upward, and the
width in 522, always downward. Two mechanisms:

- **Phantom empty trailing line.** Text ending in whitespace: at the wider
  constraint the trailing space fits; at `width()` it overflows, is hung, and
  a `Regular` break starts a line that then commits empty. `finish_line`
  copies the previous line's metrics onto it and pushes an empty
  `LineItemData`, defeating the empty-last-line height guard. Ahem 16px,
  `"aaa bbb ccc "`: 3 lines / 48px becomes 4 lines / 64px. Reachable content —
  `normalize_runs` emits a trailing `' '` when a collapsed run ends in
  whitespace.
- **ULP-premature break.** `fl(fl(x + w) − w)` can land one unit below `x`, so
  the last cluster that fit no longer satisfies `<=`. Helvetica 17.3px, the
  pangram at `max_advance` 34: the widest line `"over "` splits into `"ove"` +
  `"r "`, 12 lines becomes 13.

A non-zero `text-indent` amplifies both, because `layout_width` adds
`indent.max(0)` while the breaker subtracts the indent per line: Helvetica
23.7px with `text-indent: 40px` at `max_advance` 139 goes from 4 lines/94.8px
to 5 lines/118.5px with a completely different break set.

The consequence is a **probe/commit disagreement**: the measure pass reports
one height and the commit another for the same box, so a parent sizes a box
from a measurement its own commit then contradicts.

This branch reproduces it exactly — it is what ships today, and correcting it
changes rendering. §8 D7 states the options.

---

## 5. The bounded inline formatting context

`text` now receives a UA-only internal block-flow display. Its flat children
are collected in source order into one paragraph: text nodes become styled
runs, `display: contents` and transparent inline-flow wrappers recurse, the
four `inline-flex`/`inline-grid`/`inline-linear`/`inline-relative` modes become
indivisible Parley inline boxes, and absolute/fixed children become zero-sized
static-position markers. Every atomic box still runs its ordinary Hughie inner
algorithm before Parley decides whether its complete margin box fits the
remaining line width.

`raw-text` carriers directly under `text` or one `wrapper` deep dissolve into
that paragraph and carry per-run `preserve-breaks`. The UA sheet makes the
supported `view`/`image`/nested-`text` child paths atomic `inline-flex` boxes.
An arbitrary span tag still matches no content opt-in and is suppressed; a
nested `text` is an atomic inner paragraph rather than web-elements'
transparent rich-text span. `text-maxline` truncation remains absent.

### 5.1 `TextBrush = ()` is the deepest incompatibility

`pub type TextBrush = ();` (`crates/hughie/src/style/text.rs`). Parley's
`Layout<B>` stores `styles: Vec<Style<B>>` with `Style<B> { brush, underline:
Option<Decoration<B>>, strikethrough: Option<Decoration<B>> }`. The brush is
the only channel through which run-level identity survives into the retained
layout and out to the painter. With `()` there is none: `push_style` returns an
index that `TextMeasurer::shape` computes and immediately discards, and that
call is the single place where a run↔style mapping currently exists.

So painting resolves everything once per paint item from one element's
`ComputedValues` (`crates/dom/src/paint/text.rs`): the fill colour and its
gradient, `text-shadow` and its `currentColor` resolution, the
`-webkit-text-stroke` pair, and the propagated decoration set — plus
`font-size` for the ink-extent heuristic in the walker, and `visibility` and
`pointer-events` in the paint-order build. Every one of those is per-run in a
real IFC; Lynx sets colour, font, letter-spacing, decoration, decoration
thickness and `-x-text-decoration-width`/`-gap` per run via `PushTextStyle`,
and pushes an **event target per run**, so its hit-testing granularity is the
run, not the element.

The incompatibility is not large today — dom's entire Parley surface is one
import, `TextLayout` is `pub(crate)`, and nine sites name the brush type. It
deepens with every inline feature layered on top, which is the argument for
settling it before the ownership move rather than after.

### 5.2 The proposal: brush carries identity, not appearance

`TextBrush` becomes an opaque run index — `#[derive(Clone, Copy, PartialEq,
Debug, Default)] pub struct RunStyleIndex(u32)`, which satisfies Parley's
blanket `Brush` bound (`Clone + PartialEq + Default + Debug`) trivially. The
host supplies it per run (a `TextRunStyle::brush()` accessor defaulting to
`RunStyleIndex::default()`), `translate_run_style` sets it, and the painter
reads `glyph_run.style().brush` and resolves it against a per-paragraph table.

The table should hold the **originating element's `NodeId`** for each run, not
a style snapshot. The painter then calls the existing `Document::paint_style`,
and the decoration walk (`propagated_decorations`) starts from the run's own
element — which is what makes the propagated set per-run rather than just its
values.

**Why identity and not appearance.** The obvious alternative — and what
Parley's own examples do — is `TextBrush = peniko::Brush`, i.e. put the colour
in the layout. That would make `color` a *layout* input: changing it would
mutate the retained layout's style table, so the artifact would have to be
rebuilt, and `color` would acquire layout damage. Stylo classifies `color` and
`text-shadow` as repaint-only, and §3 depends on that: the eviction path is
gated on `needs_relayout`, so a colour flip never reaches a shaped layout at
all — pinned by
`a_repaint_only_restyle_never_reaches_the_text_eviction_path`. An index leaves
the shaped layout bit-identical under a colour change and re-runs only the
painter, which is exactly the class stylo already assigns it. The same
reasoning covers `text-shadow`, `-webkit-text-stroke` and decoration
colour/style.

Two traps:

- `Default for RunStyleIndex` is `0`, a *valid* index — there is no unset
  sentinel, and `translate_run_style`'s `..ParleyTextStyle::default()` spread
  currently supplies `brush`, so it must stop covering it.
- `push_style` asserts `style_index <= u16::MAX` and does not dedupe. That cap
  applies to any route.

**The zero-churn alternative.** `Glyph::style_index: u16` is public,
`ClusterData` carries it (so it is shaping output and survives a rebreak), and
`GlyphRunIter` already splits glyph runs on style-index change. A host-side
table keyed by Parley's own index therefore works with `TextBrush = ()`
unchanged, reached through `glyph_run.positioned_glyphs().next()`. It costs no
type churn and no bytes; it costs a dependency on Parley's internal
`push_style` ordering and a less direct read (`GlyphRun` exposes neither the
index nor a text range). §8 D1.

---

## 6. Paragraph artifact ownership

The unit of line breaking is the paragraph, so a Flow element owns the retained
layout assembled from all of its runs and atoms. Standalone text leaves keep
their existing node-owned artifacts; this preserves the cheap leaf path used
inside Flex/Grid/Linear/Relative containers.

The ownership consequences are explicit:

- **Invalidation granularity.** A run edit walks through boxless/transparent
  wrappers and clears the Flow owner. Empty-cache early stopping remains for
  hidden, detached, and ordinary boxed subtrees.
- **Paint and hit granularity.** `visual/build.rs` emits one paragraph
  `PaintItemKind::TextRun`; source text records are suppressed. Until
  `TextBrush` carries run identity, the Flow owner supplies paint style and the
  whole paragraph is one hit target.
- **`background-clip: text`** collects one silhouette per paragraph.
- **Slot recycling.** The artifact dies with the element's slot instead of the
  text node's.

### 6.1 Landed and remaining stages

1. **Container/run style split: landed.** `TextContainerView` carries
   paragraph inputs; each run's `ComputedValues` carries its font inputs,
   `line-height`, and `letter-spacing`.
2. **Flow ownership and multiple shaping runs: landed.** The host walks flat
   children, the Flow element owns the layout, and source-text invalidation is
   routed to it.
3. **Atomic inline boxes: landed.** Parley places already-measured Hughie
   boxes, followed by a line-metrics pass that accounts for atom baselines and
   descenders.
4. **Brush identity: pending.** An opaque run index must survive shaping so
   paint, decorations, visibility, pointer events, and hits can resolve per
   originating element without turning repaint-only style into layout damage.
5. **Transparent rich-text spans and truncation: pending.** Nested `text` is
   currently atomic, and `text-maxline` still needs a paragraph-level cut.

---

## 7. Why `text-maxline` truncation follows paragraph ownership

1. **It is a paragraph property.** In Lynx it is an element *attribute*, not
   CSS (absent from the 236-entry property table), carried as
   `TextProps::text_max_line` and forwarded as the paragraph style
   `kTextPropTextMaxLine` through `SetParagraphStyle`. Its unit of effect is
   "the Nth line of the paragraph". The Flow-owned retained layout now supplies
   that unit; a standalone text leaf remains a one-run paragraph.
2. **The cut mutates content.** The inspectable reference implementation
   (`lynx-stack`'s `XTextTruncation.ts`) physically truncates the DOM text
   nodes and appends a literal `"..."`, after binary-searching the last
   retained line. Which run holds the cut is a paragraph-level answer; deciding
   it per artifact is a different, wrong feature.
3. **It is a measurement input.** A maxline cut changes the measured height and
   can change the width, so `max_lines` has to join the `BreakConstraint` key
   in §1.1. The existing three-entry paragraph memo is the place to add it.
4. **The custom truncation child is an inline atom.** `inline-truncation`
   builds a sub-paragraph whose measured width feeds the search for the cut
   point. That needs both the landed IFC and a future inline-truncation atom
   plus the paragraph-level cut in step 5.
5. **The fast path is a trap.** Web-core's fast path (`-webkit-line-clamp` +
   `text-overflow: ellipsis`, taken when there is no custom truncation child
   and `tail-color-convert != "false"`) *could* be done per-artifact — but only
   correctly when the paragraph is a single run, which is the state the
   ownership move exists to leave. Implementing it first buys a feature that
   must be redone.

Note also that `text-overflow` in Lynx is only `clip | ellipsis` (no
`<string>`), and `-webkit-line-clamp`/`line-clamp` are not author-facing in
either target, so the fast path is an implementation detail of the attribute
rather than a property to expose.

---

## 8. Decisions

Marked with a recommendation where the evidence supports one. None of these are
taken.

- **D1 — brush representation.** (a) `TextBrush = RunStyleIndex(u32)` newtype,
  or (b) keep `TextBrush = ()` and key a host table on Parley's own
  `style_index`. *Recommend (a)*: it is the documented channel, survives
  `Layout::clone`, and reads off `GlyphRun::style()` directly. (b) is free but
  couples to Parley's `push_style` ordering.
- **D2 — what the index resolves to.** The run's originating element `NodeId`,
  resolved through `Document::paint_style` at paint time, or an
  `Arc<ComputedValues>` snapshot held on the artifact. *Recommend the
  `NodeId`*: it keeps every repaint-only property out of the artifact, which is
  §5.2's whole argument, and pins no style snapshot.
- **D3 — ownership move shape.** The staged route in §6.1 (steps 2 and 3
  separately shippable, each a no-op), or one move as PR #90 does. *Recommend
  staged*, because step 4 is the only behaviour change and it is worth
  isolating.
- **D4 — `line-height` scope.** Lynx applies `line-height` per **paragraph**
  (`SetParagraphStyle`); W3C applies it per inline box. This is a bucket-1
  versus bucket-2 classification with a real rendering consequence, so per
  `AGENTS.md` it is a question for the user rather than a silent choice.
- **D5 — truncation semantics.** Confirm `text-maxline` lands after step 3, and
  choose between the fast path (line-clamp semantics) and the slow path
  (physical cut, appended `"..."`, and the `layout` event carrying `lineCount`
  and per-line `{start, end, ellipsisCount}`). The slow path is what a custom
  `inline-truncation` child requires.
- **D6 — draft PR #90.** See §9.5. *Recommend rebase-and-fold* over
  land-then-rework.
- **D7 — the shrink-to-fit defect (§4).** Three options, all rendering changes:
  (i) leave it; (ii) shrink to `full_width()` instead of `width()` — verified an
  exact no-op over 21 018 un-indented shrink-eligible cases, still 87/7 881
  divergent with a non-zero indent, but it shifts right/centre alignment by the
  widest line's trailing whitespace; (iii) skip the shrink entirely for
  `Alignment::Left` in LTR — provably invisible to alignment offsets and to
  paint, but it makes the line count depend on `text-align`. A clean fix needs
  an alignment width independent of the break width, which Parley 0.11 does not
  expose; that is an upstream request.

---

## 9. Findings recorded in passing

### 9.1 The box cache is over-specified for leaves

`Cache`'s key includes the available height, and for a text node the answer
does not depend on it. That is why nine measurements reach a flex text child
per pass rather than three. The text-level memo absorbs it, but the general
case — any leaf whose measurement is independent of one axis — still pays a
cache miss and a full `compute_leaf_layout_with_measurement`.

### 9.2 Single-axis measurements can never hit the committed slot

`packed_inputs_match` lets a stored `Commit` answer a `Commit` or a
`Measure(Both)`, never a `Measure(Horizontal|Vertical)`. Flexbox issues
single-axis probes for *every* item (the flex base size and the automatic
minimum size), so a flex text child always enters the measurer even when its
committed answer would serve. Under the old design that also forced the deep
clone. Whether a commit slot should answer a single-axis request is a
cache-semantics question worth its own look.

### 9.3 Text under `display: linear` never wraps

In `compute_linear_layout`, an in-flow item with an auto main size and
`linear-weight: 0` is measured with `AvailableSpace::MaxContent` on the main
axis even when the container's main size is definite, and `commit_in_flow` then
commits at that same max-content measurement clamped only by the item's own
min/max. Linear has no shrink pass, so a text leaf is shaped and committed
unwrapped regardless of the container width. Out of scope here; recorded for
the layout owner.

### 9.4 Committed-layout consumers remain bounded

Flow commit reads its owner's committed layout to place atomic inline boxes;
visual construction reads the same artifact to emit a paragraph record, and
paint reads it for the text run and `background-clip: text` silhouettes. Hit
testing still recovers only the element id from `PaintItemKind::TextRun`; it
does not inspect glyphs or line fragments. Scroll and events do not consume
the text artifact, and there is no accessibility module. Every layout probe
must therefore still be restored before the commit/visual/paint consumers run.

### 9.5 PR #90 (`codex/inline-layout`)

Scope, per its own body: the four atomic `inline-*` modes only — no
author-facing `display: inline`, no `inline-block`, no inline-span
fragmentation. It keeps Lynx's initial `display: flex`; a UA-origin-only
`display: block` entry supplies the private Flow paragraph for `text`. Standard
flex/grid item blockification remains unchanged.

Where it lands relative to §5–§6:

- **Brush untouched.** It generalises run styles for *shaping* only
  (`impl TextRunStyle for ComputedValues`, and a `collect_inline_sources` that
  threads each transparent inline ancestor's own `ComputedValues` down), so a
  collected source run shapes with its own font but **paints with the flow
  element's colour, shadow, stroke and decorations**. That remains the gap
  specified in §5.2.
- **Ownership moves** to the element for flow boxes, with
  `clear_inline_text_nodes` zeroing the absorbed text nodes, and per-text-node
  artifacts kept under non-flow parents.
- **One retained layout stays.** Three break constraints are memoized around
  the committed/resting state. Atom topology participates in the shape
  fingerprint; same-topology metric changes update Parley's boxes in place,
  clear break memos, and avoid reshaping.
- **Real inline boxes**: `parley::InlineBox` plus a host-supplied
  `AtomicInlineBox`, a `resolve_atomic_line_metrics` pass recomputing per-line
  ascent/descent and storing a per-line block offset used by layout and paint.

The rebase onto the single-slot retained-text architecture is complete: probes
restore the committed constraint and atom snapshot at pass tail, while a
committed same-width atom change refreshes positions without a second shape.
