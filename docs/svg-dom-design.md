# SVG as DOM cascaded by stylo: design (2026-11-23)

Every SVG image the engine draws is an SVG DOM subtree that stylo cascades
and the engine records into vector drawing commands: the Lynx
`<svg content|src>` component, `<image src="x.svg">`, `background-image` and
`mask-image`. An `svg` element in a document is an SVG DOM subtree too, and
draws nothing yet. This document describes the design as
built in `crates/terminus/src/svg/`, `crates/terminus/src/visual/svg.rs` and
`crates/terminus/src/paint/svg.rs`. It
supersedes the converter of `docs/svg-lynx-component-design.md` sections 7
and 8's producer half (that document keeps its history and says which
sections are superseded).

## 1. Rulings

| Date | Ruling |
| --- | --- |
| 2026-11-23 | Coverage: one implementation for every entry. An `svg` element, the Lynx `<svg content|src>`, `<image src>` and CSS `url()` images all go through the SVG DOM and stylo. |
| 2026-11-23 | Naming: the tag `svg` is `terminus`'s built-in SVG element; the Lynx component is `lynx-svg`, and `bobcat-core` maps the Lynx tag `svg` to `lynx-svg` when it creates the element (`crates/bobcat-core/src/main/tree/tag_map.rs`). |
| 2026-11-23 | Animation: CSS only (`animation`, `@keyframes`, `transition`); no SMIL. |
| 2026-11-23 | CSS inside an SVG document is read (`style` elements, `style` attributes), through stylo. This reverses the 2026-10-09 ruling "CSS inside SVG is not read" of `docs/svg-lynx-component-design.md`, whose reason was that a second styling engine was unacceptable. |
| 2026-11-24 | XML parsing and XML-to-DOM are a capability of the tree (`tree/xml/`) with no SVG vocabulary, implemented only as far as SVG needs. |
| 2026-11-24 | An SVG image's recording is drawn by a paint-item plan and the paint walk, without box layout: the image's root is not a CSS box, the image's own encode loop is removed, and nothing is culled for an image. |

## 2. How other engines do it

- Gecko: SVG elements are DOM nodes styled by its one style system; an SVG image (`<img>`, CSS `url()`) is a separate document of the same engine (`SVGDocumentWrapper`), shared by every use of the URL.
- Chromium: the same structure; an SVG image is an isolated `SVGImage` holding its own `Page` and document, styled and laid out by Blink.
- Blitz: hands both inline `<svg>` and SVG images to `usvg`, and its DOM nodes under an `svg` do not paint.
- This engine follows Gecko and Chromium: an SVG image is an isolated `terminus` document.
- Unlike both browsers, the XML parse runs in the host's decode pool, off the document thread, as a bitmap decode does.

## 3. The pipeline

1. **Parse** (host thread). `ImageEvent::parse_document(source, bytes, DocumentKind::Svg)` makes a `VectorDocument` (`svg/image.rs`): the parsed `XmlMarkup` and the natural size from the root's `width`, `height` and `viewBox` (CSS Images 3 default sizing, at least one px per axis). `Send + Sync`; the host reports it through `ImageReports::parsed_document`. Bytes that do not parse report `Failed`. The parse is the tree's XML parser (`tree/xml/`, `XmlMarkup::parse`), which has no SVG vocabulary; `VectorDocument::parse` passes it the SVG-image policy that an element in no namespace is an SVG element, and then requires the root to be the SVG `svg` element. Element nesting is not bounded before `roxmltree` parses the bytes; the walk that builds the `XmlMarkup` keeps no element deeper than `MAX_NESTING` (256), with its subtree (section 8, "Risk: element nesting is not bounded before the XML parse").
2. **Apply** (document thread). `Document::apply_image_events` passes each report to `Document::apply_image_event` (`svg/image.rs`), which hands `ImageRegistry::apply` a closure that turns a `VectorDocument` into a `VectorImage`: `IsolatedSvg::build`, then `IsolatedSvg::image`. `IsolatedSvg::build` makes a `Document<()>` with a device the size of the natural size, the SVG user-agent sheet and the image sheet mounted, the one `svg` element built under its document element (`append_svg_markup`, which mounts the markup's `style` elements as author sheets and instantiates `use` elements), and its styles flushed, its timeline starting at the host document's animation clock. `IsolatedSvg::image` then records the subtree with `RootGroup::Recorded`, shapes its text with the host document's `TextContext`, so SVG glyphs come from the page's fonts and `@font-face` registrations, and files the recording in the isolated document's own side table (`Document::keep_svg_recording`, `svg/present.rs`). `Document::plan_svg_image` plans it (`visual/svg.rs`) under the identity map, with no root box item before the items and no clip, space, scroll slot or animation slot, and the paint walk paints that plan into one display list with culling off (`paint::walker::walk_uncultured`): every recorded item is drawn, one outside the viewport included, since a `meet` destination shows what lies outside the `viewBox`. The image's root is not a CSS box: no layout, render or stacking-context build runs for the isolated document. It is dropped after the paint unless it still runs an animation (section 6); a caller that keeps it asks `IsolatedSvg::image` again.
3. **Draw**. The registry entry holds the `VectorImage` (the painted display list, viewport, `preserveAspectRatio`, a process-unique key). The paint walk emits one `VectorDraw` per visible tile, and the painter's raster cache bakes it (`docs/svg-lynx-component-design.md` section 8).

**The image sheet** (`svg/ua.rs`, `IMAGE_RULES`): `:root { font-family:
sans-serif; }`. The text shaper turns an empty family list into the generic
`sans-serif` (`seldon::text::line`, `family_list`), which is what the earlier
converter gave text with no `font-family`; stylo's initial `font-family` is
`serif`. With the rule, such text resolves to `[sans-serif]` and shapes with
the same family as before. `currentcolor` at the root resolves against the
isolated document's initial `color`, black.

## 4. The DOM surface

- An element is in the HTML or the SVG namespace (`NodeFlags` bit 13). `Document::create_element("svg")` makes the SVG `svg` element; `Document::create_svg_element` is the DOM's `createElementNS` for the SVG namespace; `Node::is_svg_element` reads the namespace. All three are on `terminus::link` with no feature.
- Presentation attributes become presentational hints (`svg/presentation.rs`), below author rules and the `style` attribute.
- Every SVG `svg` element is replaced content (`svg/root.rs`); its descendants generate no box.
- `Document::add_svg_user_agent_stylesheet` mounts the SVG user-agent sheet (`transform-origin: 0 0` on every SVG element but an outermost `svg`).
- `tree/xml/`: XML bytes to an owned `XmlMarkup` (`XmlMarkup::parse`) and an `XmlMarkup` to DOM nodes (`Document::append_xml`), the tree's counterpart of a browser's XML parser. It keeps elements of the namespaces the tree builds from XML (SVG), attributes in no namespace plus `xml:*` and `xlink:*`, and every text node.
- `svg/build.rs`, `svg/use_element.rs`: `Document::append_svg_markup` builds an `XmlMarkup` with `append_xml`, mounts its `style` elements as document sheets and gives each `use` a closed shadow root holding a clone of its target (resolved through `href`, else `xlink:href`). These are crate-private; the image pipeline is their caller.

## 5. Recording

`svg::record::record(document, root, group)` walks a flushed subtree and
produces a `Recording` (`svg/recording.rs`): `Item`s in paint order, in
viewport units, every transform absolute, every paint a peniko brush, text
collected but unshaped, and every item but a pop naming the element it was
recorded from (`node`: the shape or `text` element, or the element whose
group effects open a layer). Values come from computed style (paint, stroke,
opacity, visibility, display, `clip-path: url()`, `mask-image`, fonts,
`transform` with `transform-origin` against the box `transform-box` names)
and, for geometry that is no CSS property, from attributes
(`svg/attributes.rs`). The full list of elements, values and drawing rules is
`svg/record.rs`'s module documentation. `svg::text::shape` shapes the
recording's text with `seldon::text::shape_line`, stores the glyph runs in
each text item and grows every bounds layer by the text inside it. A shaped
recording reaches a display list only through the visual plan and the paint
walk (section 3).

The regression corpus is `svg/record_tests.rs`: each document goes through
`VectorDocument::parse`, `IsolatedSvg::build` and `IsolatedSvg::record` or
`IsolatedSvg::image`, the production functions.

## 6. Animation

SVG animation is CSS only (`animation`, `@keyframes`, `transition`), driven
by the document's stylo animation driver (`crates/terminus/src/style/animation.rs`).
SMIL elements (`animate`, `animateTransform`, `animateMotion`, `set`) are
unknown elements: they draw nothing and change nothing, so the static state
draws.

**An SVG image.** Applying a parsed document
(`Document::apply_image_event`, `crate::svg::image`) builds its isolated
document with its timeline starting at the host document's animation clock,
so an animation the first flush creates starts at that instant. If the
document then has a running or pending animation or transition
(`IsolatedSvg::runs`), the host keeps it in `Document::animated_images`, by
source, an empty and unallocated table on a page without one; otherwise it is
dropped as before. Nothing in an image can start a new animation (no script,
no interaction), so a document that has none never will.

- **Ticking.** `Document::advance_animations(now)` advances every kept document that is in use with the same `now`. A tick that restyled something records the document again, replaces the registry entry's `VectorImage` (a new key, so the painter's raster cache bakes it again) and notes a visual mutation. A document whose animations have all finished leaves the table at that tick; the last recorded image stays.
- **In use.** Presented by a replaced box (the registry entry's presenter count) or drawn by the frame being committed: `FramePainter::paint` hands the walk's vector draws to `Document::note_drawn_images`, which compares each kept entry's current key with them. That is one emptiness test per paint on a page without an animated image, and nothing per draw. An image nothing uses is not ticked and requests no frame; when it is used again its next tick samples the current clock.
- **Frames.** While an image in use runs, `Document::has_active_animations`, `AnimationTick::needs_next_frame` and the committed frame's `animations_active` and `needs_main_ticks` are true. The embedder reads the frame's two flags: `needs_main_ticks` decides whether the painter posts a frame to main (`crates/bobcat-core/src/paint/lib.rs`, `begin_frame`), and `animations_active` makes the painter owe a frame (`Painter::is_animating`, behind `owes_frame`).
- **Synthetic sources.** A `svg-content:` entry leaves the registry with its last presenter, and its kept document with it.
- **Cost.** One kept document for the `ICON` document of `crates/terminus/tests/svg_images.rs` with a spinning group holds about 70 KB (71 807 bytes live after the apply against 3 076 for the static document, measured with a counting allocator, release build). A page without one pays one emptiness test per `advance_animations` and one per paint.

## 7. What was removed

Relative to `main`:

- The converter's walk (`render/svg/parse.rs`, `Converter`, `parse`) and its style resolver (`render/svg/style.rs`: `Style`, `Declarations`, the presentation-attribute property table), which read `roxmltree` nodes directly.
- The `roxmltree`-reading half of the gradient code (`gradient`, `stops`, `href`, `inherited_color`) and the per-document gradient cache, a documented approximation: a gradient now resolves for each reference.
- The image encode loop (`render/svg/encode.rs`, `encode`), which turned a parsed document's items into a display list: an SVG image's recording is now planned and painted by the paint walk (section 3).
- The directory `render/svg/` itself. Its surviving parts are in `svg/` (the shape builders `svg/shapes.rs`, the gradient brush `svg/paint_server.rs`, the text item types `svg/text.rs`, the image types `svg/image.rs`, the corpus `svg/record_tests.rs`) and in `paint/svg.rs` (drawing a recorded item).
- The nesting pre-scan (`render/svg/nesting.rs`, `nesting::bound`), which cut every element nested deeper than `MAX_NESTING` out of the markup before `roxmltree` parsed it and refused a document whose entities could nest markup past that bound (`TooDeep`). Removed by a user decision of 2026-11-25; section 8, "Risk: element nesting is not bounded before the XML parse", records what it prevented.

## 8. Limits

- **Risk: element nesting is not bounded before the XML parse.** Nothing limits how deeply an SVG document's elements nest before `roxmltree` 0.21.1 parses it (`tree/xml/mod.rs`, `with_xml`). `roxmltree` parses an element's content by recursion, one level per nested element, and bounds only entity expansion (ten references inside one another), not element nesting. Measured with plain `<g>` nesting inside an `svg` root, on a thread with a 2 MiB stack, by bisecting the depth in a child process per attempt (2026-11-25, `nightly-2026-04-20`, macOS arm64): the release profile (thin LTO, `codegen-units = 1`) takes about 690 bytes a level and overflows past 3,061 levels; the dev and test profiles, with `roxmltree` at `opt-level = 3` (root `Cargo.toml`), take about 740 bytes a level and overflow past 2,860 levels. A native host parses on tokio's blocking pool (`crates/bobcat-resources/src/images.rs:748`, `parse_job`, through `crates/bobcat-resources/src/executor.rs:203`, `spawn_blocking`, on the runtime built at `executor.rs:145` with tokio's default 2 MiB thread stack). In the browser the parse runs in the Render Worker, the thread that instantiates the module (`images.rs:818`, `spawn_parse`, and `images.rs:874`, `load_async`), whose stack is the 1 MiB the `wasm32-unknown-unknown` target gives the linker (`-z stack-size=1048576`; `.cargo/config.toml` sets no other); the depth at which it overflows there was not measured. A stack overflow aborts the process natively and traps the instance in the browser, and neither can be caught. An SVG document nested deeper than that, from any source (`<image src>`, `background-image`, `mask-image`, the `lynx-svg` component's `src` or `content`), therefore takes the process down. `MAX_NESTING` does not prevent it: it bounds the walk over `roxmltree`'s finished tree, and what is built and recorded after it, not `roxmltree`'s own parse. The pre-scan that prevented it on `main` (`render/svg/nesting.rs`, section 7) was removed by a user decision of 2026-11-25. Two ways would close it: a parser that does not recurse (an event reader with a depth counter in the tree's own loop), or a nesting limit inside `roxmltree`.
- Applying an SVG image costs 23.0–24.0 µs for the `ICON` document and 30.6–31.6 µs for the `TILE` document of `tests/svg_images.rs` on the document thread (building the isolated document, flushing its styles, recording, planning and painting; release build, medians of 1000 iterations, three runs, 2026-11-24), and the host-side parse 3–4 µs. The converter on `main` encoded a parsed document in 1–2 µs and parsed it in 8–13 µs. Accepted for now. Since the XML layer keeps white-space text nodes, a pretty-printed document builds a text node between every two elements: `ICON` goes from 5 to 10 nodes and `TILE` from 9 to 19, and the medians of alternating runs on one machine (2026-11-24) moved from 25.0 to 26.9 µs (`ICON`) and from 31.3 to 36.0 µs (`TILE`); the minified form of `ICON` stays at 25–26 µs, and the parse at 2.5–3.7 µs.

- `filter`, `mask`, `pattern`, `marker`, `image`, `textPath`, per-character text positions, `rotate`, `textLength`, `dominant-baseline`, bidi reordering within a chunk and `mix-blend-mode` are not recorded; an element with a mask draws nothing.
- A `clipPath` with several children clips with their concatenation under `nonzero`, which differs from their union where children overlap with opposite winding. A `clip-path` basic shape on an SVG element is not recorded.
- Text has no bounding box before shaping, so `transform-box: fill-box` on a text-only element uses the `view-box` reference.
- A bounds-shaped compositing layer (a group with `opacity` and no single-shape clip path) is sized by its items' path bounds grown by the join's reach alone (`svg/layers.rs`, `items_bounds`, `stroke_reach(.., 1.0)`) and by the text's advance boxes (`svg/text.rs`), so the corner of a square cap on a diagonal line, and a text stroke or a glyph overhang, can be cut at the layer's edge. The converter on `main` sized the layer the same way.
- A `@font-face` registered after an image was drawn does not re-shape it.
- An SVG image's display list is placed through the plan's world matrices, which are f32: a draw's transform differs from the recorded f64 one by that rounding (a stroked path at (0, 10) with a 4-unit bevel stroke draws under a translation by (4.8e-8, -1.9e-7) where its recorded transform is the identity), far below a device pixel.
- SMIL animation is not supported: `animate`, `animateTransform`, `animateMotion` and `set` are unknown elements, and the static state draws.
- An animated SVG image records its document again and the painter bakes its texture again after every tick that changes it.
- A gradient inside a `display: none` subtree has unstyled stops, which record as black.
- XML content builds SVG-namespace elements only: `XmlNamespace` has one variant, because the tree has elements for the HTML and SVG namespaces and the SVG work needs only the second.
- Comments and processing instructions in XML content make no node; the tree has no such node types.
- A namespaced attribute other than `xml:*` and `xlink:*` is dropped, because the tree's attributes have no namespace. `xlink:href` is kept under that name, and the SVG layer reads it only when `href` is absent.
- The XML-to-DOM entry (`Document::append_xml`) is crate-private: `style` and `use` are applied by the SVG builder (`Document::append_svg_markup`), not by the elements, so a subtree built with `append_xml` alone is not a complete SVG subtree.
