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
[Native ↔ web conflicts](#native--web-conflicts) lists every disagreement found.
One of them — the truncation marker gate — was put to the user and decided
*against* the default: see [A0](#a0-truncation-marker-gating--ruled-not-a-gap).

## The suite

| File | Cases | Passing | Gap-ignored |
| --- | --- | --- | --- |
| [`crates/hughie/tests/web_text_replication.rs`](../../crates/hughie/tests/web_text_replication.rs) | paragraph algorithm: clamping, cut points, tail fitting, word-break | 16 | 3 |
| [`crates/bobcat-core/src/main/tree/web_text_replication.rs`](../../crates/bobcat-core/src/main/tree/web_text_replication.rs) | the `<text>` element, its UA sheet and its attributes | 28 | 9 |
| [`crates/bobcat-core/src/main/runtime/web_text_replication.rs`](../../crates/bobcat-core/src/main/runtime/web_text_replication.rs) | the Element PAPI: content, restyle, truncation, `setNativeProps`, layout events | 15 | 6 |
| [`crates/dom/tests/web_text_replication.rs`](../../crates/dom/tests/web_text_replication.rs) | relayout, percentage sizing, scroll targets, glyph and atom paint | 12 | 3 |
| [`crates/dom/tests/web_text_screenshots.rs`](../../crates/dom/tests/web_text_screenshots.rs) | golden screenshots — the originals' own oracle, for the cases whose claim is visual | 10 | 0 |
| [`crates/bobcat-source/tests/web_text_css_replication.rs`](../../crates/bobcat-source/tests/web_text_css_replication.rs) | text CSS across the `.web.bundle` wire | 9 | 0 |
| **Total** | | **90** | **21** |

A gap-ignored test asserts the `web-core` behavior and is marked
`#[ignore = "GAP: …"]` naming the cause with a `file:line`. It is a real
assertion, never weakened — run any file with `-- --ignored` and every one of
the 21 fails on the assertion its own string names, so no ignore is masking a
test that would now pass. Three of the 21 are marked `DEVIATION` instead: they
assert `web-core` against a ruling that this engine follows native Lynx, and
are listed in [F](#f-recorded-deviations-not-gaps).

### Screenshots

The lynx-stack originals are overwhelmingly Playwright full-page screenshot
diffs at `maxDiffPixelRatio: 0`. Metric assertions are the right oracle for
layout but show nothing about what the text *looks* like, so
`web_text_screenshots.rs` restores the originals' oracle for the ten cases whose
claim is visual — per-run colour on one line, mixed sizes on one baseline, an
atomic box drawn inline, a gradient running down real letterforms, a literal
newline breaking where it is written. Refresh with
`FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test web_text_screenshots`.

**A golden is committed only where the frame is right.** Where a case renders
wrongly today it appears here only if the fixture can be built so the defective
element is absent; a case whose whole frame is wrong gets no test in that file
at all, because an `#[ignore]`d screenshot test is self-healing —
`assert_golden` writes a missing PNG and fails only on that first run
(`crates/flashbulb/src/golden.rs:100-103`), so the next run would pass against a
golden of the defect. Those visual claims are carried by `#[ignore]`d metric
siblings instead.

### Where the originals came from

230 cases were catalogued across `web-elements/tests/web-elements.spec.ts`
(`x-text`, `x-textarea`, and the text-adjacent `layout`/`scroll-view` cases),
`web-core-e2e/tests/reactlynx.spec.ts`, `web-core/tests/*`, and the compiled
cards under `web-tests/dist/`. 85 became the 109 tests above — a case splits
where it carries independent claims, and a visual case is replicated twice,
once as a metric and once as a golden. The remaining 145 are out of scope, for
the reasons in [Not replicated](#not-replicated).

## What closed since the first assessment

The suite was first written against the branch point 6cac1d42 and scored 55
passing / 35 gap-ignored. Re-run unchanged against 22d9ac1c, 24 commits later,
it scored **67 / 23**: twelve gaps closed, no regressions. (The table above
reads 77 / 32 because a later round added the screenshots and closed the
coverage holes below; the twelve closures are the engine delta.)

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

## Gaps and ruled deviations

### A0. Truncation marker gating — ruled, not a gap

**Ruled (user, 2026-09-14): the engine's gating is the intended behavior.**
`crates/hughie/src/text/block/truncate.rs:95` (the maxline retreat) and `:135`
(the `Tail` decision) both guard on `style.overflow == TextOverflow::Ellipsis`.
`text-overflow`'s initial value is `clip` and the Lynx UA sheet never declares
it, so a bare `text-maxline` or `text-maxlength` clamps with **no marker**; with
`text-overflow: ellipsis` the marker appears. Do not "fix" this.

The nine tests that were gap-ignored on it now assert it, each in two passes —
one under the fixture's own declarations and one with `text-overflow: ellipsis`
— so both sides of the gate stay covered and neither pass can hold vacuously.

What the references do, kept on record in every one of those tests:

| Attribute | native Lynx | `web-core` | this engine |
| --- | --- | --- | --- |
| `text-maxline` | gates on `text-overflow` | **unconditional** (`x-text.css:216-230,:239-241`; `XTextTruncation.ts:349-366`, which never reads it) | gates — **matches native, diverges from `web-core`** |
| `text-maxlength` | **unconditional** (Android `TextRenderer.java:126-135`: "Ellipsis will be appended disregarding the overflowing mode.") | **unconditional** (`x-text.css:191-194`) | gates — **matches neither reference** |

`AGENTS.md` names `web-core` as the compatibility target, so the maxline row is a
knowing divergence from it and the maxlength row is a Lynx-vello-specific
behavior. Both are deliberate outcomes of the ruling, not oversights, and both
reverse by deleting one condition.

CSS `text-overflow: ellipsis` as the real W3C single-line overflow marker is a
**different** feature, and it is implemented in its own right — see
[A2](#a-truncation--what-remains-open) below, now closed: an overflowing
`white-space: nowrap` line is cut at the clip edge with neither Lynx attribute
taking part. The engine therefore reaches the marker by two independent routes,
and un-conflating them stays correct under this ruling.

### A. Truncation — what remains open

| # | Gap | Cause |
| --- | --- | --- |
| A1 | `tail-color-convert` is unparsed | `truncate.rs:226` picks the run holding the last visible byte, which is the `="false"` behavior applied unconditionally; the default path should take the block's own style. |
| A4 | `ellipsize-mode` is inert | `text.rs:26-31` — `apply_attribute_style` matches only `text-maxline` and `text-maxlength`. Carried by no test of its own. |

**A2 — the overflow-driven ellipsis path — is closed.** A `white-space: nowrap`
line wider than its measure is cut at the clip edge under
`text-overflow: ellipsis`, which is css-ui `text-overflow` in its own right:
neither `text-maxline` nor `text-maxlength` takes part, and the clamp path
never saw the case because the one line had consumed all of its source.
`TextBlock::overflow_cut`
([`crates/hughie/src/text/block/mod.rs:776-874`](../../crates/hughie/src/text/block/mod.rs))
compares the line's visible advance against the constraint, keeps the widest
prefix that still leaves room for the dots — shaped once in the run at that
boundary by `measure_dots` (`:876-897`) — and hands the result to
`truncate::plan` (`mod.rs:645`) as a third cut candidate beside the two clamps
(`crates/hughie/src/text/block/truncate.rs:28-40,132-140`), so `ellipsis_count`,
`truncated()` and the existing `CutPlan` path all hold unchanged. With
truncation content present the cut retreats until the freed width covers it;
under `text-overflow: clip` nothing is cut and the line simply overflows, which
is what the property asks for.

The path is restricted to a nowrap paragraph whose breaking left **exactly one
line**. A cut drops every line past the one it falls in, which is right for the
single unbroken line `nowrap` normally produces and wrong for the several a
preserved newline can still leave, so that shape is left uncut rather than half
served. `an_overflowing_nowrap_line_is_marked_by_text_overflow_ellipsis`
(`crates/hughie/tests/web_text_replication.rs:1247-1284`) is un-ignored and
passes.

### B. Atomic inline boxes — what #227 did not fix

| # | Gap | Cause |
| --- | --- | --- |
| B1 | An atom's origin **omits** border+padding | `crates/dom/src/layout/text_block.rs:548` writes the paragraph-space origin into `location`, which every reader takes as border-box relative. The atom lands **short** by the content-box inset: `location == (0,0)` where the origin is `(10,10)` under 10px padding. |
| B2 | An atom's `margin` never reaches the line | `text_block.rs` hands the block the atom's **border** box, so `margin-left: 50px` on an inline image adds nothing to the advance (142 where the reference gives 192). Padding works, because `box-sizing: border-box` folds it in. |
| B3 | A `display: contents` wrapper leaves the atom below it with a zero box | The post-placement hide loop at `text_block.rs:590-601` exempts only slots that are themselves in `atoms`; a wrapper holding an atom is not exempt, so `hide_subtree` zeroes the atom the paragraph placed. |

**B4 is closed.** The frame builder used not to descend past a text block's
paragraph when that block was the paint root or its own stacking context:
`build_stacking_context` pushed the paragraph and returned, so an atomic inline
box or an out-of-flow child inside it was never emitted, while the in-context
path did descend. It now pushes the paragraph and falls through to the same
collection walk (`crates/dom/src/visual/build.rs:563-569`), which is also the
in-context order — element box, paragraph, then the descent. The glyphs stay
unique because `collect_child` (`crates/dom/src/visual/build.rs:829-871`) drops
text nodes and the layout slots `place_and_hide`
(`crates/dom/src/layout/text_block.rs:503`) hid, so an absorbed nested scope
reaches no second record.
`a_boxed_child_paints_from_a_text_block_that_is_its_own_context`
(`crates/dom/tests/web_text_replication.rs:986`) now runs in CI.

### C. Shaping

| # | Gap | Cause |
| --- | --- | --- |
| C1 | A run's `line-height` is taken from the style of the **next** span | Upstream parley: `shape/mod.rs:127-128` advances `item.style_index` before flushing the pending item, so the last span's `line-height` governs every line. Observed as a truncation flow's `line-height` rewriting lines *before* the cut. |
| C2 | Leading collapsible whitespace is emitted, not removed | `crates/hughie/src/text/block/content.rs:359` — `PendingKind::Space => true` emits unconditionally with no start-of-line suppression. |

### D. Paint — closed

A gradient-valued `color` on a **nested** run used to be ignored: the tile the
ramp filled from was one paragraph-level decision taken from the establishing
element's own style, so a solid-coloured block resolved no tile at all and every
nested run's gradient fell back to a solid fill.

The tile is now per run, because `color` is a per-run property
(`crates/dom/src/paint/text.rs:107-118`, `:187-241`): the establishing element
keeps its padding box, and a nested element gets the union of its own line
fragments — each fragment's advance horizontally, its line box vertically. That
is the area `web-core`'s `color: transparent; background-clip: text` rewrite
paints over (`packages/web-platform/web-core/src/style_transformer/rules.rs:259-291`).
A nested scope that only *inherits* the gradient gets a union of its own rather
than the ancestor's tile, which is also what `web-core` does:
`--lynx-text-bg-color: inherit` plus `background-image: var(--lynx-text-bg-color)`
applies on every nested `x-text` (`x-text.css:7-31`).

Asserted by `a_gradient_color_on_a_nested_run_fills_only_that_run`
(`crates/dom/tests/web_text_replication.rs:714`), which was the gap-ignored test
for this row and now runs.

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
  hidden. This is §D.15's recorded exception (`deviations.md:492-498`) — Lynx's
  inline-ness is structural, not cascaded. `web-core` does let `display: none`
  win, so the exception is wider than the structural argument alone requires:
  `display: none` generates no box at all and so cannot break an inline-ness
  invariant. **Ruled (user, 2026-09-14): `display: none` on a `text` stays
  unsupported for now**, and the exception keeps its current width. The replica
  `display_none_removes_a_text_block_and_an_inline_run_alike` stays `#[ignore]`d
  and marked DEVIATION rather than GAP, so the divergence stays visible and the
  ruling is reversible by narrowing one UA declaration.
- **A3. `text-maxline="1"` is a one-line clamp, not a one-line shape.**
  **Ruled (user, 2026-09-15): this engine follows native Lynx here.** Android
  builds the `StaticLayout` at the available width and calls `setMaxLines(1)`
  (`TextRenderer.shouldBeSingleLine()`,
  `lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/shadow/text/TextRenderer.java:181-184,231-245`),
  and iOS gives a container of the same size a `maximumNumberOfLines`
  (`lynx/platform/darwin/ios/lynx/shadow_node/text/LynxTextRenderer.m:1009-1031`),
  with the tail ellipsize reached only under `text-overflow: ellipsis`. So
  `text-maxline="1"` clamps the paragraph as it normally wraps, and neither
  `white-space: nowrap` nor a fill-available cap is forced. `web-core`'s
  `x-text[text-maxline="1"]` pair (`x-text.css:216-241`) — `nowrap` plus
  `max-width: -webkit-fill-available` — is therefore **not** replicated, and a
  one-line clamp stops at the last word boundary that fit rather than running
  to the parent's edge. The 09-14 marker ruling ([A0](#a0-truncation-marker-gating--ruled-not-a-gap))
  is unaffected: the marker stays gated on `text-overflow`. Two replicas assert
  `web-core` and stay `#[ignore]`d, marked DEVIATION rather than GAP —
  `a_single_line_clamp_caps_the_block_to_its_parent_s_available_width` (the
  88px cap) and
  `a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word`
  (the 384 ink), both in
  `crates/bobcat-core/src/main/tree/web_text_replication.rs`. Their passing
  sibling `a_one_line_clamp_keeps_the_nested_run_s_colour_and_the_parent_s_weight`
  keeps the card's style claim in CI.
- `var()` inside an `@font-face` descriptor is not substituted. Correct per
  css-variables-1 §3; the browser `web-core` runs on behaves identically.
- `x-text`, `inline-image` and `inline-text` are `web-core`'s *HTML* mappings of
  Lynx tags, not Lynx tags. The PAPI mints `text`, `image` and `raw-text`.

## Coverage holes — closed

A re-assessment found four places where the suite reported green on something it
did not actually cover. All four are now closed; they are recorded because the
failure mode is easy to reintroduce.

- **Custom `<inline-truncation>` was carried by zero ignored tests**, and
  `truncation_content_is_skipped_entirely_when_no_maxline_is_declared` passed for
  a reason `web-core` does not share: `web-core` skips the subtree because the
  block is not in the overflowing-maxline state, whereas here
  `inline-truncation { display: none }` is unconditional, so the test would still
  have passed with the whole feature deleted. That negative case is kept and now
  says so; its **positive twin** — the same paragraph with a `text-maxline` it
  overflows, where the content must be laid in at the clamp — is what pins the
  feature, and fails today. Three more replicas carry the same wiring gap at the
  tree layer and one at the dom layer.
- **`@font-face` → shaping** is now carried by an ignored test in the dom file,
  and the `bobcat-source` module doc states that its nine green tests cover the
  wire and lowering only.
- **`scrollIntoView` genuinely cannot be carried by a test.** There is no entry
  point in `crates/dom/src/scroll/`, so a test could only re-implement CSSOM-View
  inside itself and assert nothing about the engine. It stays report-only until
  an entry point exists; the two `a_text_block_is_a_*_axis_scroll_target_like_a_view`
  tests pin scroll-target geometry and say explicitly that they do not pin
  alignment.
- **Four cases were writable but declined.** #230 implemented
  `__SetDataset`/`__GetDataset`/`__AddDataset` and a BTS `SelectorQuery` whose
  `setNativeProps` is serviced by `__BobcatQueryNodes`, so the four
  `text/set-native-props-*` cases are now written against the BTS harness. They
  are ignored on two real gaps: the push is not retargeted onto a leading
  `raw-text` child the way `web-core` retargets it, and the attribute written
  instead replaces the element's children where `web-core` appends. Still
  genuinely absent: a direct MTS selector PAPI, `__AddClass`, and any rect field
  in `nodeFields`, so the geometry half of the SelectorQuery case stays blocked.

## Native ↔ web conflicts

`AGENTS.md` resolves these to `web-core` by default. Conflicts 1 and 7 were put
to the user and decided the other way; the rest stand as `web-core`.

### 1. Truncation marker gating

| | Behavior | Evidence |
| --- | --- | --- |
| native | The `text-maxline` ellipsis appears **only** under `text-overflow: ellipsis`; the default `clip` clamps with no tail | Harmony `text_shadow_node.cc:63-66,:162-164`; Android `TextRenderer.java:185-200`; iOS `LynxTextRenderer.m:1013-1021` |
| web-core | **Unconditional.** `text-overflow` is never read in JS; maxline ≥ 2 uses `-webkit-line-clamp`, maxline = 1 has the stylesheet declare `text-overflow: ellipsis` itself | `x-text.css:207-210,:216-230,:239-241`; `XTextTruncation.ts:349-366` |
| lynx-vello | Gated — **matches native** | `truncate.rs:95,:135` |

**Decided (user, 2026-09-14): the gating stays.** So on `text-maxline` this
engine follows native rather than the stated compatibility target, and on
`text-maxlength` — which both references leave ungated — it follows neither. See
[A0](#a0-truncation-marker-gating--ruled-not-a-gap).

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

### 7. `text-maxline="1"` geometry

| | Behavior | Evidence |
| --- | --- | --- |
| native | A one-line clamp of the paragraph as it normally wraps: the layout is built at the available width and told to keep one line | Android `TextRenderer.java:181-184,231-245`; iOS `LynxTextRenderer.m:1009-1031` |
| web-core | A one-line *shape*: `white-space: nowrap` on the inner box and `max-width: -webkit-fill-available` on the host, so the line never breaks and runs to the parent's edge | `x-text.css:216-241` |
| lynx-vello | Clamped — **matches native** | `crates/hughie/src/text/block/mod.rs` truncation path; the UA sheet declares neither |

**Decided (user, 2026-09-15): the clamp stays.** See
[F](#f-recorded-deviations-not-gaps).

## Not replicated

- **`x-textarea` / `x-input` (75 cases).** Blocked on an editable-text
  subsystem that does not exist. **Ruled out of scope (user, 2026-09-14): not
  to be built.** The catalogue stands as their specification if that reverses;
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
