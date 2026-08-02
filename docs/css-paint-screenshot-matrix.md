# CSS paint screenshot matrix

This document records the browser-referenced CSS paint matrix in
`crates/dom/tests/css_atlas.rs`, how its references were produced, and the
known differences found by the first complete audit.

## Scope and oracle

The matrix contains exactly 1,000 named Rust tests and 1,000 unique fragments.
Every case is an independent 128×128 document containing only `<div>` elements;
the 160 text cases add plain text nodes inside those divs. The generator asserts
name and fragment uniqueness and rejects any generated fragment containing an
element other than `div`. Structural divs explicitly use flex layout because
flow/block layout is not yet implemented by Hughie.

The references were captured from Chrome 150.0.7871.187 through Playwright at
DPR 1, in sRGB, with a fixed 640×640 viewport. Text cases use the same vendored
Ahem font in Chrome and Parley; a shard fails readiness unless Chrome reports
that the face loaded. Each browser shard contains 25 isolated 128×128 iframes.
In full-audit mode, the native side likewise paints 25 independent
`dom::Document`s and appends their scenes into isolated cells before one GPU
readback. A normal run preserves the original index/slot topology but does not
build the 189 ignored cases. A permanent pixel test checks a non-zero
translated cell against a standalone render byte-for-byte and verifies that
outset effects do not leak into either adjacent cell.

The ordinary suite has three explicit expectation states:

| State | Cases | Checked reference |
| --- | ---: | --- |
| `BrowserMatch` | 666 | Chromium PNG under `tests/screenshots/css-paint/` |
| `NativeSnapshot` | 145 | DOM/Parley PNG under `tests/screenshots/css-paint-native/` |
| `Skip` | 189 | No PNG and `#[ignore]`; retained for full browser audit |

The 145 native snapshots are all audited W3C-correct Chromium differences:
84 rasterization or boundary/subpixel-sampling cases plus 61 cases where CSS
permits different UA geometry, pattern, color, or `auto` metrics. They are
active regressions: their current standards-conforming DOM/Parley result
must remain stable even though it is not required to match Chromium. The full
browser audit still compares every one of these cases to Chromium so a parity
change cannot silently alter its standards disposition.

Every one of the 334 Chromium differences has a standalone HTML fixture under
`crates/dom/tests/fixtures/css-paint-differences/` and a row in
`crates/dom/tests/css-paint-differences.tsv`. Thus “difference” is not
synonymous with “ignored”: the registry is the union of 145 active native
snapshots and 189 skips. The asset inventory requires exact basename equality
for the 666 browser PNGs, 145 native PNGs, and 334 difference fixtures.

`FLASHBULB_UPDATE_SNAPSHOTS` is rejected by the entire atlas suite. Browser
references can only come from the Playwright split workflow. Native references
can only be written through the filtered `CSS_PAINT_UPDATE_NATIVE=1` workflow
below, which cannot write the browser directory. Comparison in both active
states uses Flashbulb's Playwright/pixelmatch-compatible defaults: YIQ threshold
0.2, anti-alias detection enabled, and a zero non-antialiased-pixel budget.

Skipped cases are not permissive xfails: they perform no ordinary pixel
comparison and are not indirectly rendered merely because an active neighbor
shares their atlas shard. Re-auditing is a separate, explicit workflow that
temporarily captures all 1,000 browser references, includes ignored tests, and
rebuilds the two-column case-to-issue difference registry.

The generated cases cite the WPT area that informed each family. They are
small, deterministic probes adapted to the current pure-div DOM/layout surface,
not verbatim copies of WPT files.

## Reference repository survey

Counts below are pinned to the local reference checkouts used for this audit.
“Physical PNG” counts every browser/platform baseline file. “Logical assertion”
removes the platform suffix for an otherwise identical test. WPT is counted
with its own manifest parser: a reftest source document can expand to more than
one test URL through `?variant`.

| Repository | Revision | Directly reusable visual tests |
| --- | --- | --- |
| Lynx | `66b002855a25a5a8812fe878af69e20a346d0408` | 8 integration baseline PNGs, or 4 logical image assertions after merging the Android/iOS copies: `image`, `layout_linear`, `list_base`, and `text_flattern_element`. Three Clay performance-overlay golden-test definitions exist (60/90/120 fps), but this checkout has none of their PNG baselines, so they are not counted as usable references. |
| lynx-stack | `216b1b3adbd3b139a32f953f9d40b87c806f0b26` | 1,296 Playwright baseline PNGs: 624 in `web-core-e2e` and 672 in `web-elements`. Removing the Chromium/Firefox/WebKit Linux suffix leaves 502 logical screenshot assertions. Physical browser coverage is Chromium 501, Firefox 406, WebKit 389. |
| WPT | `e04cee8384c069f6bb7dd54f920ef9395a5e22f5` | 26,897 reftest source documents: 26,473 ordinary reftests plus 424 print reftests. Variants expand these to 27,069 manifest test URLs: 26,645 ordinary plus 424 print. |

The Lynx count includes only PNGs actually loaded by
`testing/integration_test/test_script/lib/test_runner/mixin/img_diff_mixin.py`;
ordinary product, devtool, and explorer images are not baselines. The
lynx-stack count includes the two tracked `*-snapshots` directories under
`packages/web-platform/web-core-e2e/tests` and
`packages/web-platform/web-elements/tests`.

Conservative feature-name subsets in the Lynx-family repositories are:

| Area | Lynx logical / PNG | lynx-stack logical / PNG |
| --- | ---: | ---: |
| Paint order / stacking | 0 / 0 | 1 / 3 |
| Mask | 0 / 0 | 0 / 0 |
| Background | 0 / 0 | 3 / 9 |
| Border | 0 / 0 | 4 / 12 |
| Shadow | 0 / 0 | 1 / 3 |
| Filter | 0 / 0 | 6 / 18 |
| Clip | 0 / 0 | 1 / 3 |
| Transform | 0 / 0 | 0 / 0 |
| Text | 1 / 2 | 95 / 281 |

The lynx-stack subset counts use feature names in snapshot paths, avoiding the
large false positive that would result from counting every page that happens to
use a colored background or border. They overlap and are not totals.

For WPT, 82,097 tracked HTML/XML/SVG candidates were parsed through
`tools/manifest/sourcefile.py::SourceFile`. The manifest classification, rather
than filename or PNG counting, excludes `resources`, `support`, `tools`,
manual/visual-only documents, and reference-only files. Relevant non-exclusive
subsets of the 26,897 source documents are:

| WPT area | Reftest source documents |
| --- | ---: |
| Paint order, z-index, and stacking paths | 154 |
| `css/css-masking` | 471 |
| Modern `css/css-backgrounds` | 701 |
| Backgrounds including CSS2 suites | 1,037 |
| Modern `css/css-borders` | 65 |
| Borders including CSS2 suites | 571 |
| `css/css-shadow` | 111 |
| `css/filter-effects` | 306 |
| Strict clip paths in masking/overflow/CSS2 visual effects | 457 |
| `css/css-transforms` | 791 |
| Core `css-text` plus `css-text-decor` | 1,773 |
| Extended text stack including fonts, inline, writing modes, ruby, and CSS2 text/fonts | 4,193 |

These WPT subsets overlap and must not be summed.

## Coverage and first audit

| Area | Cases | Pixelmatch-exact | Audited mismatch |
| --- | ---: | ---: | ---: |
| Backgrounds and gradients | 120 | 36 | 84 |
| Borders | 100 | 48 | 52 |
| Shadows and outlines | 80 | 69 | 11 |
| Paint/stacking order | 100 | 100 | 0 |
| Overflow and paint containment | 80 | 75 | 5 |
| Transforms | 100 | 100 | 0 |
| Filters and opacity | 80 | 70 | 10 |
| Clip paths | 80 | 47 | 33 |
| Masks | 100 | 46 | 54 |
| Text | 160 | 75 | 85 |
| **Total** | **1,000** | **666** | **334** |

“Pixelmatch-exact” means zero non-antialiased pixels beyond the Playwright
threshold. The 334 mismatches are not one undifferentiated bug count:

| Classification | Cases | Meaning |
| --- | ---: | --- |
| W3C-correct — rasterization or boundary sampling | 84 | The standard behavior is present; Vello/Parley and Chromium cover a boundary or subpixel differently. |
| W3C-correct — permitted UA choice | 61 | CSS deliberately leaves the observed geometry, pattern, color, or `auto` metric to the UA. |
| **W3C-correct subtotal** | **145** | These are Chromium-parity differences, not standards defects. |
| Definite W3C gap | 170 | A standard grammar or required behavior is missing or implemented incorrectly. |
| Non-W3C compatibility | 19 | `text-stroke` is a WebKit/Lynx compatibility surface, not a W3C CSS property. |
| **Audited mismatch total** | **334** | Exactly the difference registry: 145 native snapshots plus 189 skips. |

The 84 raster/sample cases are 8 hard-stop boundary samples, 48 general
edge-coverage cases, and 28 ordinary text subpixel cases. The 61 UA-choice
cases are 16 dash/dot patterns, 24 3D border colors, 16 rounded double borders,
and the five `line-through` cases using
`text-decoration-thickness: auto`. Conversely, the corresponding five
`line-through` mismatches with an explicit `3px` thickness are part of the 170
definite W3C gaps.

The browser stage div explicitly uses `isolation: isolate`, matching the native
document element's initial stacking-context role. With that oracle alignment,
all 22 negative-z probes now match Chromium exactly, and the complete
paint/stacking-order family is 100/100 exact.

Other strong control results include 100/100 transform cases, 60/60 overflow
and nested-overflow cases, 20/20 opacity groups, 20/20 clip ellipses, 10/10
`clip-path: path()`, and 50/50 outset/inset shadow cases.

## Audited difference inventory

| Issue | Class | Cases | Finding and implementation evidence |
| --- | --- | ---: | --- |
| `css-gradient-multi-position-stops` | W3C gap | 76 | The Lynx Stylo build compiles out second positions and interpolation hints in `vendor/stylo/style/values/specified/image.rs`; the whole declaration is rejected before the painter sees it. |
| `css-gradient-hard-stop-boundary-sampling` | W3C-correct: raster/sample | 8 | Coincident stops reach the painter, while the boundary scanline selects a different device sample in `crates/dom/src/paint/background.rs`. |
| `css-border-dash-dot-pattern` | W3C-correct: UA choice | 16 | DOM's painter uses fixed 2w/1w dashed and 2w dotted periods in `crates/dom/src/paint/border.rs`; CSS does not prescribe dash length, spacing, or perimeter phasing. |
| `css-border-3d-light-face-color` | W3C-correct: UA choice | 24 | `groove`, `ridge`, `inset`, and `outset` use the painter's fixed lighten/darken colors. CSS specifies the visual relationship, not a color formula. |
| `css-double-border-rounded-corners` | W3C-correct: UA choice | 16 | DOM's one-third line allocation and rounded-corner interpolation differ from Chrome, but CSS does not fully determine the line split or rounded style transition. |
| `css-outline-nonsolid-styles` | W3C gap | 6 | `double`, `groove`, and `ridge` outlines collapse to a solid ring even though CSS UI gives outline styles the corresponding border-style meanings. |
| `vello-chromium-edge-coverage` | W3C-correct: raster/sample | 48 | Circle, mask, hard-edge gradient, and one shadow boundaries differ by small coverage sets. The atlas-isolation control is byte-identical, so the atlas is not the cause. |
| `css-position-static-grammar` | W3C gap | 5 | The Lynx `position` parser omits standard `static`. A rejected later declaration leaves an earlier absolute declaration active and moves the containment probe. |
| `css-filter-brightness-over-one-approximation` | W3C gap | 1 | `brightness(2)` uses a screen-blend approximation rather than the specified filter transfer function. |
| `css-filter-blur-offscreen-pass` | W3C gap | 9 | `filter: blur()` is ignored because the required offscreen texture pass is not implemented. |
| `stylo-lynx-clip-path-geometry-box-grammar` | W3C gap | 10 | DOM's painter can resolve border/padding/content reference boxes, but the Lynx `clip-path` parser accepts only a basic shape and rejects a trailing geometry-box keyword. |
| `pulsar-clip-inset-radius-percent-reference-box` | W3C gap | 2 | Percent radii in `inset(... round ...)` resolve against the post-inset rectangle; they must use the original reference box before overlap normalization. |
| `stylo-lynx-clip-polygon-grammar` | W3C gap | 10 | DOM has a polygon painter, but `polygon()` is absent from the Lynx allowed-basic-shapes parser set, so it is unreachable. |
| `pulsar-mask-multiple-layer-composite` | W3C gap | 12 | Only the first non-`none` mask layer paints and `mask-composite` is ignored. |
| `pulsar-mask-luminance-mode` | W3C gap | 8 | `mask-mode: luminance` is treated as alpha in the current `SrcIn` sandwich. |
| `text-overflow-wrap-break-word-policy` | W3C gap | 8 | Hughie's Parley translation hard-codes `OverflowWrap::BreakWord` instead of the standard `normal` initial value, changing default wrapping and some `background-clip: text` extents. |
| `stylo-lynx-repeating-gradient-grammar-scope` | W3C gap | 4 | `repeating-linear-gradient()` and the px stop form used by these probes are rejected by the Lynx grammar. The blank result is not a repeat-paint failure, but it remains a missing standard CSS surface under this project's W3C-first policy. |
| `stylo-lynx-text-shadow-list-grammar` | W3C gap | 4 | Lynx Stylo declares `text-shadow` as `single_item`, so standard comma-separated shadow lists never reach the painter. |
| `pulsar-text-shadow-blur` | W3C gap | 10 | Text-shadow offset and color paint, but the parsed blur radius is ignored. |
| `css-text-decoration-auto-thickness-ua-choice` | W3C-correct: UA choice | 5 | `text-decoration-{002,006,010,014,018}` use `line-through` with `text-decoration-thickness: auto`; CSS leaves the resulting font/UA metric and exact line geometry to the UA. |
| `stylo-lynx-text-decoration-thickness-grammar` | W3C gap | 5 | `text-decoration-{003,007,011,015,019}` explicitly request `3px`, but the Lynx build omits that authorable longhand and the painter always uses font metrics. |
| `pulsar-text-stroke-join-geometry` | Non-W3C compatibility | 19 | Every `text-stroke-000..019` case except matching control `006`: `text-stroke`/`-webkit-text-stroke` is a WebKit/Lynx extension, not a W3C CSS property. Kurbo's default join differs from Chromium's glyph-stroke join and unhinted Vello coverage contributes remaining pixels. |
| `css-text-subpixel-rasterization` | W3C-correct: raster/sample | 28 | Ordinary Ahem glyph/baseline coverage differs at subpixel edges with Vello glyph hinting disabled. The text is present and standards behavior is not missing. |

The authoritative difference mapping is the two-column
`crates/dom/tests/css-paint-differences.tsv` (`case-name`, `issue`). The
name-based classifier refuses an unclassified mismatch, a duplicate row or
name, index/name drift, a missing case, or a mapped case that now matches. It
also asserts the four disposition totals (84, 61, 170, and 19), so the
145-case W3C-correct subtotal cannot change accidentally. The generator maps
all 145 W3C-correct rows (84 raster/sample plus 61 UA-choice) to
`NativeSnapshot`; every other registry row maps to `Skip`.

## Regeneration and audit

Generate Rust cases and the browser HTML shards:

```sh
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --html-output output/playwright/css-paint
```

Serve the repository root over loopback, then capture all 40 640×640 shards
with the committed Playwright context (requires the `playwright` npm package
and local Chrome):

```sh
python3 -m http.server 8765 --bind 127.0.0.1
node crates/dom/tests/support/capture_css_paint_references.mjs \
  http://127.0.0.1:8765 output/playwright/css-paint/atlases
```

Inspect the atlases, then split them into the committed browser-reference
directory. The default split writes only the 666 `BrowserMatch` tiles; it never
writes either the native-reference directory or any other difference:

```sh
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --split-atlases output/playwright/css-paint/atlases
```

To intentionally accept the current DOM/Parley behavior for all 145
W3C-correct Chromium differences, run only the generated `css_native_` tests
on a real GPU:

```sh
CSS_PAINT_UPDATE_NATIVE=1 \
  cargo test -p dom --test css_atlas css_native_
```

This command is deliberately incompatible with audit mode and with
`FLASHBULB_UPDATE_SNAPSHOTS`. The test-name filter also excludes the inventory
test, preventing it from racing the parallel snapshot writes. Inspect all
updated PNGs, then validate the complete asset partition:

```sh
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --prune-reference-assets --validate-assets
```

The checked regression suite compares 811 screenshot cases: 666 to Chromium
and 145 to native snapshots. With the inventory test, Cargo reports 812 passed
and 189 ignored; the native atlas
builder does not render those skipped cases in a normal run:

```sh
cargo test -p dom --test css_atlas
```

To re-audit after renderer changes, reuse the 40 captured atlases but split all
1,000 Chromium references into a fresh temporary directory.
`--include-differences` is deliberately legal only with
`--reference-output` outside `tests/screenshots`, so a full audit cannot
overwrite either committed browser or native PNGs:

```sh
CSS_PAINT_AUDIT_REFS="$(mktemp -d)"
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --split-atlases output/playwright/css-paint/atlases \
  --reference-output "$CSS_PAINT_AUDIT_REFS" \
  --include-differences
CSS_PAINT_AUDIT=/tmp/css-paint-audit.tsv \
  CSS_PAINT_REFERENCE_DIR="$CSS_PAINT_AUDIT_REFS" \
  cargo test -p dom --test css_atlas -- --include-ignored
python3 crates/dom/tests/support/classify_css_paint_audit.py \
  /tmp/css-paint-audit.tsv
```

Audit mode routes all three states—including `NativeSnapshot`—to the temporary
Chromium references, renders all 1,000 cases, and requires a complete 1,000-row
TSV. If a mapped case now matches, a new mismatch appears, or any disposition
total changes, the classifier stops before replacing
`css-paint-differences.tsv`; review the mapping and its standards disposition
explicitly, then rerun it.

After classification, first regenerate Rust and HTML metadata, then update the
native W3C-correct set and split the browser matches. Finally validate the
666/145/189 asset partition:

```sh
python3 crates/dom/tests/support/generate_css_paint_cases.py
CSS_PAINT_UPDATE_NATIVE=1 \
  cargo test -p dom --test css_atlas css_native_
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --split-atlases output/playwright/css-paint/atlases
python3 crates/dom/tests/support/generate_css_paint_cases.py \
  --prune-reference-assets --validate-assets
```

The temporary directory named by `CSS_PAINT_AUDIT_REFS` is disposable after
the re-audit.
