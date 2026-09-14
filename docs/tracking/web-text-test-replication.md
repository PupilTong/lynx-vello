# Replicating lynx-stack's web-platform text tests

Status of the native port of every text-related test in `lynx-stack`'s
`packages/web-platform`, and the engine gaps that port exposes.

Unlike its neighbours in this directory, this file is a **status tracker**, not
research: every row below is backed by a test in this repository that runs
today. It supplements [`css-text.md`](css-text.md), which stays the property
survey.

Last re-assessed against `main` at 22d9ac1c (2026-09-14).

## The reference is `web-core`

[`AGENTS.md`](../../AGENTS.md) sets the compatibility target: a `.web.bundle`
must "render and behave the same as [it does] under `web-core` today", and
explicitly *not* by "reimplementing Android/iOS native platform code paths".
Where native Lynx and `web-core` disagree, the replicas assert `web-core`.
[Native ↔ web conflicts](#native--web-conflicts) lists every disagreement found,
because one of them is a place where this engine currently matches *native* and
is therefore scored as a gap.

## The suite

| File | Cases | Passing | Gap-ignored |
| --- | --- | --- | --- |
| [`crates/hughie/tests/web_text_replication.rs`](../../crates/hughie/tests/web_text_replication.rs) | paragraph algorithm: clamping, cut points, tail fitting, word-break | 9 | 10 |
| [`crates/bobcat-core/src/main/tree/web_text_replication.rs`](../../crates/bobcat-core/src/main/tree/web_text_replication.rs) | the `<text>` element, its UA sheet and its attributes | 26 | 7 |
| [`crates/bobcat-core/src/main/runtime/web_text_replication.rs`](../../crates/bobcat-core/src/main/runtime/web_text_replication.rs) | the Element PAPI: content, restyle, truncation, layout events | 13 | 3 |
| [`crates/dom/tests/web_text_replication.rs`](../../crates/dom/tests/web_text_replication.rs) | relayout, percentage sizing, scroll targets, glyph and atom paint | 10 | 3 |
| [`crates/bobcat-source/tests/web_text_css_replication.rs`](../../crates/bobcat-source/tests/web_text_css_replication.rs) | text CSS across the `.web.bundle` wire | 9 | 0 |
| **Total** | | **67** | **23** |

A gap-ignored test asserts the `web-core` behavior and is marked
`#[ignore = "GAP: …"]` naming the cause with a `file:line`. It is a real
assertion, never weakened — run any file with `-- --ignored` and every one of
the 23 fails on the assertion its own string names, so no ignore is masking a
test that would now pass.

### Where the originals came from

230 cases were catalogued across `web-elements/tests/web-elements.spec.ts`
(`x-text`, `x-textarea`, and the text-adjacent `layout`/`scroll-view` cases),
`web-core-e2e/tests/reactlynx.spec.ts`, `web-core/tests/*`, and the compiled
cards under `web-tests/dist/`. 85 became the 90 tests above, a few cases
splitting where they carried independent claims. The remaining 145 are out of
scope, for the reasons in [Not replicated](#not-replicated).

## What closed since the first assessment

The suite was first written against the branch point 6cac1d42 and scored 55
passing / 35 gap-ignored. Re-run against 22d9ac1c, 24 commits later, it scores
**67 / 23**: twelve gaps closed, no regressions.

**All twelve trace to one commit**, d19cbea2 *"feat(dom): render element text
content and migrate raw-text to CSS" (#227)*, in two clauses:

- `text[text] { content: attr(text); }` added to the UA sheet
  (`crates/bobcat-core/src/main/tree/text.rs:95`) closed the **largest gap in the
  original assessment**: a `text` attribute written on a `<text>` element used to
  be inert, and since the ReactLynx compiler collapses a static text child into
  exactly that call, a typical card's whole body measured `0×0`. Four closures.
- The `input.goal.commits()` branch into `hughie::compute::compute_inline_box_layout`
  (`crates/dom/src/layout/text_block.rs:436-444`) closed the **atom-never-committed**
  defect: an atomic inline box used to be laid out under `LayoutInput::measure`,
  which writes no layout, so `place_and_hide` copied an unwritten zero and every
  `<view>`/`<image>` inside a `<text>` ended the pass `0×0`. Eight closures.

30632acb (#231), the other candidate, closed nothing here — its only effect was
to invalidate half of one gap's cited cause (see B3 below). None of #211–#226 or
#228–#235 changed a single result.

## Gaps

### A. Truncation — the largest remaining cluster

| # | Gap | Cause |
| --- | --- | --- |
| A1 | A bare `text-maxline` clamp emits no marker | `crates/hughie/src/text/block/truncate.rs:95` guards the retreat on `TextOverflow::Ellipsis`, whose initial value is `clip` and which the Lynx UA sheet never declares. `web-core` is unconditional. **This engine matches native here — see [conflict 1](#1-truncation-marker-gating).** |
| A2 | A `text-maxlength` cut emits no three-dot tail | `truncate.rs:135` (decision spans `:127-146`) — same gate. `web-core` appends `::after { content: "..." }` unconditionally, and so does native, so this half is a plain gap. |
| A3 | `tail-color-convert` is unparsed | `truncate.rs:226` picks the run holding the last visible byte, which is the `="false"` behavior applied unconditionally; the default path should take the block's own style. |
| A4 | No overflow-driven ellipsis path | `crates/hughie/src/text/block/mod.rs:625` — a cut needs a maxline clamp or a maxlength cut, so CSS `text-overflow: ellipsis` on an overflowing `nowrap` line marks nothing. This is the genuinely-W3C half, distinct from the Lynx attributes. |
| A5 | No `text-maxline="1"` nowrap / fill-available treatment | `crates/bobcat-core/src/main/tree/text.rs:91-101` has no counterpart to `web-core`'s `x-text.css:216-241`, so a one-line clamp breaks at a word boundary instead of running to the parent's edge. Measured 288 where the reference gives 384. |
| A6 | `ellipsize-mode` is inert | `text.rs:26-31` — `apply_attribute_style` matches only `text-maxline` and `text-maxlength`. Compounds A1/A2; carried by no test of its own. |

### B. Atomic inline boxes — what #227 did not fix

| # | Gap | Cause |
| --- | --- | --- |
| B1 | An atom's origin **omits** border+padding | `crates/dom/src/layout/text_block.rs:548` writes the paragraph-space origin into `location`, which every reader takes as border-box relative. The atom lands **short** by the content-box inset: `location == (0,0)` where the origin is `(10,10)` under 10px padding. |
| B2 | An atom's `margin` never reaches the line | `text_block.rs` hands the block the atom's **border** box, so `margin-left: 50px` on an inline image adds nothing to the advance (142 where the reference gives 192). Padding works, because `box-sizing: border-box` folds it in. |
| B3 | A `display: contents` wrapper leaves the atom below it with a zero box | The post-placement hide loop at `text_block.rs:590-601` exempts only slots that are themselves in `atoms`; a wrapper holding an atom is not exempt, so `hide_subtree` zeroes the atom the paragraph placed. |
| B4 | The frame builder never descends past a text block's paragraph when that block is the paint root or its own stacking context | `crates/dom/src/visual/build.rs:563-570` pushes the paragraph then returns. The in-context path does descend, so this bites only those two cases. |

### C. Shaping

| # | Gap | Cause |
| --- | --- | --- |
| C1 | A run's `line-height` is taken from the style of the **next** span | Upstream parley: `shape/mod.rs:127-128` advances `item.style_index` before flushing the pending item, so the last span's `line-height` governs every line. Observed as a truncation flow's `line-height` rewriting lines *before* the cut. |
| C2 | Leading collapsible whitespace is emitted, not removed | `crates/hughie/src/text/block/content.rs:359` — `PendingKind::Space => true` emits unconditionally with no start-of-line suppression. |

### D. Paint

A gradient-valued `color` on a **nested** run is ignored: `crates/dom/src/paint/walker.rs:816-817`
computes the gradient box only from the establishing element's own style.

### E. Absent surfaces

Each blocks replicas that could not be written at all.

- **The `layout` / `layoutchange` event.** A listener registers cleanly and never
  fires; the only engine-synthesized names are the pointer set plus `tap` and
  `longpress` (`crates/bobcat-core/src/paint/gesture.rs:81,84`). The payload
  already exists as `hughie`'s `LineInfo`
  (`crates/hughie/src/text/block/mod.rs:66-83`); what is missing is delivery and
  a host-visible query, since `Document::text_block` is `pub(crate)`
  (`crates/dom/src/layout/mod.rs:252`).
- **Custom `<inline-truncation>` content.** `crates/dom/src/layout/text_block.rs:360`
  passes `None` for `TextBlock::new`'s `truncation` parameter, and the UA sheet
  gives the tag `display: none`. hughie implements the whole algorithm, proven by
  `custom_truncation_content_replaces_the_marker_at_the_clamp`. **Carried by no
  ignored test** — see [Coverage holes](#coverage-holes).
- **`__AddClass`** and the `enableCSSSelector=false` cascade.
- **Text selection.** No selection model anywhere; `text-selection` is inert.
- **Editable text controls.** No `<input>` or `<textarea>`; 54 catalogued cases
  are specified but unwritable.
- **`scrollIntoView`** and its CSSOM-View alignment computation.
- **`@font-face` → shaping.** The rule parses, lowers and enters the cascade, but
  nothing reads it: `src:` is never fetched and no face is registered with
  parley. Native Lynx also hands the map to the platform, so this is an engine
  boundary rather than a defect — but no `@font-face` can currently change a
  glyph.

### F. Recorded deviations, not gaps

- `text { display: -lynx-text !important }` (`tree/text.rs:94`) is user-agent
  origin, so it outranks an author's `display: none` and no `text` element can be
  hidden. This is §D.15's recorded exception (`deviations.md`) — Lynx's
  inline-ness is structural, not cascaded — but `web-core` does let
  `display: none` win, so the exception is currently wider than it needs to be.
- `var()` inside an `@font-face` descriptor is not substituted. Correct per
  css-variables-1 §3; the browser `web-core` runs on behaves identically.
- `x-text`, `inline-image` and `inline-text` are `web-core`'s *HTML* mappings of
  Lynx tags, not Lynx tags. The PAPI mints `text`, `image` and `raw-text`.

## Coverage holes

Places where the suite reports green on something it does not actually cover.
These are defects in the tests, not in the engine.

- **Custom `<inline-truncation>` is carried by zero ignored tests.** Worse,
  `truncation_content_is_skipped_entirely_when_no_maxline_is_declared` passes for
  a reason `web-core` does not share: `web-core` skips the subtree because the
  block is not in the overflowing-maxline state, whereas here
  `inline-truncation { display: none }` is unconditional — the test would still
  pass with the whole feature deleted.
- **`@font-face` → shaping and `scrollIntoView`** are both open and both carried
  by zero ignored tests, so `bobcat-source` reports 9/9 green on a category whose
  downstream half does not exist.
- **Four-to-five cases are now writable that the suite still declines.** #230
  implemented `__SetDataset`/`__GetDataset`/`__AddDataset` and a BTS
  `SelectorQuery` whose `setNativeProps` is serviced by `__BobcatQueryNodes`, so
  the `text/set-native-props-*` cases can be written against the BTS harness.
  Still genuinely absent: a direct MTS selector PAPI, `__AddClass`, and any rect
  field in `nodeFields`, so the geometry half of the SelectorQuery case stays
  blocked.

## Native ↔ web conflicts

`AGENTS.md` resolves all of these to `web-core`; they are listed because the
first is a case where this engine currently implements the *native* behavior.

### 1. Truncation marker gating

| | Behavior | Evidence |
| --- | --- | --- |
| native | The `text-maxline` ellipsis appears **only** under `text-overflow: ellipsis`; the default `clip` clamps with no tail | Harmony `text_shadow_node.cc:63-66,:162-164`; Android `TextRenderer.java:185-200`; iOS `LynxTextRenderer.m:1013-1021` |
| web-core | **Unconditional.** `text-overflow` is never read in JS; maxline ≥ 2 uses `-webkit-line-clamp`, maxline = 1 has the stylesheet declare `text-overflow: ellipsis` itself | `x-text.css:207-210,:216-230,:239-241`; `XTextTruncation.ts:349-366` |
| lynx-vello | Gated — **matches native** | `truncate.rs:95,:135` |

`text-maxlength` is unconditional on both sides; only this engine gates it.
Reversing the ruling in favour of native would reclassify gap A1 as
correct-as-is, but would leave A2 a gap either way.

### 2. The ellipsis glyph

Native uses one `…` (U+2026); `web-core` uses three ASCII periods. This engine
follows the web form, with the web slow path's 3-unit retreat.

### 3. `tail-color-convert` default

Absent from the native C++ core entirely; it exists only in the Android/iOS
shadow nodes, where Android defaults it to `false`. `web-core` defaults it to
`true`. This engine reads it on neither side.

### 4. Inline image sizing

Native sizes an `<image>` in a `<text>` from its **specified** CSS width/height,
with indefinite → 0 and no intrinsic sizing; the web target resolves `auto` to
the image's intrinsic size (`css-text.md`).

### 5. `line-height` scope

Native's textra applies `line-height` paragraph-wide; the web applies it
per-run. This engine follows the web
(`crates/hughie/src/text/block/style.rs`).

### 6. The `text` attribute with children present

Native uses an element's own `text` only when `childCount == 0`, and redirects
the write into the first child when that child is a `raw-text`. `web-core`
reflects unconditionally. Since #227 this engine follows `web-core`.

## Not replicated

- **`x-textarea` / `x-input` (75 cases).** Blocked on an editable-text
  subsystem that does not exist. The catalogue stands as their specification;
  the rows worth transcribing first are `x-textarea/min-height-max-height`
  (content-driven height under min/max), `placeholder-font-size` (the
  precedence matrix), `x-input/type-value-do-not-show-input`, and
  `attribute-maxlength-change-do-not-change-value` (commit ordering).
- **`x-markdown` (36 cases).** `markdown` is a real Lynx element, but its text
  layout is delegated to a non-vendored third-party library (Serval/lynxtextra)
  through a platform `CustomMeasureFunc`, while the web implementation is an
  independent markdown-it + DOMPurify + shadow-DOM reimplementation. The two
  share no layout code, so neither is evidence about a parley-based pipeline.
  Already filed "Rare" in [`components.md`](components.md).
- **The `*-no-js` fixtures.** They exercise the browser's custom-element upgrade
  fallback (`raw-text:not(:defined)::before { content: attr(text) }`). There is
  no upgrade phase here; their rendered result duplicates
  `x-text/text-attribute-text`, which is replicated.
- **Selection screenshot cases.** Their oracle is the browser's UA `::selection`
  highlight after a Playwright `selectText()`.
- **Encoder-internal cases** — HTML tag-name remaps, `::placeholder` selector
  rewriting, SSR HTML strings, and losses inside `@lynx-js/css-serializer` that
  happen before the wire format exists.

## Reproducing

```bash
cargo test -p hughie --test web_text_replication
cargo test -p dom --test web_text_replication
cargo test -p bobcat-source --test web_text_css_replication
cargo test -p bobcat-core --lib web_text_replication
```

Append `-- --ignored` to any of them to run the gap list.
