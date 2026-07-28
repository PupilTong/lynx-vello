# High-performance DOM text rendering on wgpu/vello

Research note, 2026-07-28. Scope: how `crates/pulsar` should paint `dom` text
nodes at speed, what the wgpu/vello ecosystem offers as of today, and what — if
anything — we should adopt. Every claim about a crate here was read out of that
crate's source at the version named, not out of its README.

## Short answer

Painting glyphs *correctly* is done: `pulsar::paint::text` already drives
`Scene::draw_glyphs` from retained Parley layouts, with synthesis, decorations,
shadow and stroke passes. Painting them *cheaply* is not, and it cannot be
fixed inside vello 0.9.

Vello classic caches the **outline → path-encoding** conversion per glyph and
nothing after it. Every glyph instance is re-flattened, re-binned, re-tiled and
re-rasterized as filled vector paths on every single frame, whether or not the
text changed. Measured on this repo (below): **~1.4 ms of GPU pipeline per
frame for 1920 glyphs of 14 px body text**, with the outline cache fully warm,
on an Apple-silicon Metal device. That is a fifth of a 60 Hz budget spent
recomputing a bitwise-identical result.

The industry fix — and now the linebender fix — is a **rasterized glyph
atlas**: rasterize each (font, size, subpixel-phase) once into a texture, then
draw each instance as a textured quad. As of 2026-05-30 that exists in Rust as
[`glifo`](https://docs.rs/glifo) 0.1.1, and it is wired into `vello_cpu` and
`vello_hybrid` — **not** into `vello` 0.9.

**Recommendation:** do not switch renderers now, and do not build our own
atlas. Record the ceiling, add the benchmark that will prove any future change,
and revisit `vello_hybrid` once its glyph atlas is on by default and it clears
beta. The compatibility facts are unusually favourable when that day comes
(§5.2), so the option stays cheap to hold open.

## 1. What pulsar does today

`crates/pulsar/src/paint/text.rs` walks each committed Parley layout's lines →
`PositionedLayoutItem::GlyphRun`, and per run calls `Scene::draw_glyphs` with
the run's font, size, normalized coords, synthesis and brush. Decorations,
`text-shadow` and `text-stroke` are extra passes over the same silhouette.

That is the right shape for vello classic. The cost is in what vello does with
it afterwards.

### The one cache vello 0.9 has

`vello_encoding-0.9.0/src/glyph_cache.rs` holds a `GlyphCache` inside the
`Resolver`, which the `Renderer` owns — so it does persist across frames. Its
key is:

```rust
struct GlyphKey {
    font_id: u64, font_index: u32, glyph_id: u32,
    font_size_bits: u32,          // exact f32 bits, not quantized
    embolden_x_bits: u32, embolden_y_bits: u32, embolden_join_bits: u8,
    embolden_miter_limit_bits: u32, embolden_tolerance_bits: u32,
    style_bits: [u32; 2], hint: bool,
}
```

and its value is an `Arc<Encoding>` — the glyph's outline already converted to
vello's path-tag/path-data streams. Eviction is age-based:
`MAX_ENTRY_AGE = 64` resolve phases, `PRUNE_FREQUENCY = 64`,
`CACHED_COUNT_THRESHOLD = 256`, with a 32-entry free list of recycled
`Encoding` buffers. None of those are configurable from outside.

Two consequences worth naming:

- **`font_size_bits` is an exact `f32` comparison, not a quantization.** That is
  less of a hazard than it first looks. A *fractional* size is still a perfectly
  stable `f32`, so 13.5px hits every frame exactly as 16px does; what misses is a
  size whose **value changes**, and a `font-size` transition or animation mints a
  fresh key per frame and re-walks the outline for every glyph on screen.
  Device pixel ratio does not enter the key at all under our configuration:
  `resolve_patches` folds a uniform scale into `font_size` only when hinting is
  on, and we pass `hint(false)`, so the DPR stays in the run transform. Turning
  hinting on would pull it into the key, and a DPR change would then invalidate
  every entry.
- **What it saves is `skrifa`'s outline walk**, i.e. tens of nanoseconds per
  distinct glyph per size. It saves nothing per *instance*.

### What happens every frame regardless

`vello_encoding-0.9.0/src/resolve.rs::resolve_patches` runs per frame. Per
glyph **instance** it does a hash lookup, an `Arc::clone`, and a push; then the
resolve pass copies that glyph's path-tag and path-data streams into the packed
buffer, and accounts `glyphs.len() + 2` path tags and `glyphs.len() + 1`
transforms per run. The packed buffer is then uploaded and run through the full
compute pipeline: path-tag reduce/scan → flatten → binning → tile allocation →
coarse rasterization → fine rasterization.

In other words a paragraph of text is, to vello 0.9, a few thousand filled
Bézier paths that happen to be shaped like letters, re-submitted from scratch
every frame. There is no rasterization cache and no atlas anywhere in the crate
— confirmed by grep: `vello` 0.9's dependency list is `bytemuck`, `log`,
`peniko`, `png`, `skrifa`, `static_assertions`, `thiserror`, `vello_encoding`,
`vello_shaders`, `wgpu` 29.0.3. No `glifo`.

## 2. What it costs here, measured

800×600 headless target, 32 rows of 14 px Roboto, everything inside the
viewport so nothing is culled, release build, Apple-silicon Metal. Baseline is
the identical page with the text rows replaced by solid rectangles, which
cancels target allocation, clear and readback out of the comparison. Every
configuration renders the *same scene repeatedly*, so vello's outline cache is
maximally warm — this is steady state, not cold start.

| glyphs | `Painter::paint` (CPU encode) | GPU render + readback | Δ vs baseline | per glyph |
|-------:|------------------------------:|----------------------:|--------------:|----------:|
| 0 (rects) | 0.0043 ms | 1.905 ms | — | — |
| 640    | 0.0167 ms | 2.002 ms | +0.097 ms | 151 ns |
| 1280   | 0.0271 ms | 2.473 ms | +0.568 ms | 444 ns |
| 1920   | 0.0391 ms | 3.322 ms | +1.417 ms | 738 ns |

Read this carefully, because the two halves say opposite things:

- **Scene encoding is a non-issue.** It scales cleanly at ~18–19 ns/glyph and
  never exceeds 0.04 ms. Nothing on the CPU side of `pulsar` needs optimizing.
- **The GPU side is the whole problem, and it grows faster than the glyph
  count.** Tripling the glyphs multiplied the delta by ~14.6×. Some of that is
  glyph count and some is painted area — longer lines cover more tiles, and
  vello's fine-rasterization cost is per covered tile per path — so I would not
  claim a complexity class from three points. The direction is not in doubt.

The number that matters most is the one that does *not* appear in the table:
all 32 rows render identical text, so every glyph after the first row is a
cache hit, and it still costs 738 ns/glyph. **Caching the outline conversion
buys nothing at the instance level.** That is the ceiling, and it is structural.

## 3. Why: the three caches a text renderer needs

Framed as a taxonomy, so the options in §5 can be scored against it.

| # | Cache | What it avoids | vello 0.9 | glifo atlas | Skia/Chrome |
|---|-------|----------------|-----------|-------------|-------------|
| 1 | Shaping | Re-running HarfBuzz per frame | n/a (Parley retains) | n/a | ✓ |
| 2 | Outline → path | Re-walking `skrifa` outlines | ✓ | ✓ | ✓ |
| 3 | Rasterized coverage | Re-rasterizing pixels | ✗ | ✓ | ✓ |

We already have #1, from `dom`'s retained `TextLayout`. We get #2 free from
vello. **#3 is the missing one**, and it is the only one that scales with the
number of glyphs actually on screen rather than the number of distinct glyphs.

The reason #3 is such a large win is that glyph ink is tiny and repetitive. A
14 px 'e' is a ~10×10 px footprint that will appear hundreds of times on a
screen and thousands of times across a session, always identical. Rasterizing
its ~40 Bézier segments once and then blitting a 12×12 padded sub-image is not
a constant-factor improvement over re-rasterizing it — it changes the per-
instance work from "flatten and scan-convert a path" to "sample a texture".

## 4. The relevant history

`Plan for glyph rendering` ([vello#204], open since Nov 2022) has always listed
glyph caching as the first planned improvement, and it never landed in the
classic renderer. What happened instead is sparse strips.

Per [Linebender in 2026 Q1], the sparse-strips renderers shipped 0.0.7
alongside Vello 0.8.0, with "initial glyph caching implementation" among eight
listed optimizations, and **Vello Hybrid is "at roughly beta quality; there are
some rough edges still and performance work to be done, but it should be
usable."** The same post introduces Glifo, consolidating font-outline
extraction that had been duplicated across the Vello implementations, handling
"color emoji and atlas-based glyph caching", and notes it "should be considered
in development".

Per [vello#670], sparse strips are "the next generation of GPU path rendering",
and glyph caching is explicitly a motivation for the move — with the honest
caveat that "glyph caching becomes very inefficient as glyphs scale in size",
which is why glifo bypasses the atlas above a size threshold (§5.2).

[vello#204]: https://github.com/linebender/vello/issues/204
[vello#670]: https://github.com/linebender/vello/issues/670
[Linebender in 2026 Q1]: https://linebender.org/blog/tmil-25/

## 5. The options

### 5.1 Stay on vello 0.9 and tune what we control

Honest assessment: **there is almost nothing to win.**

- Scene reuse (`Scene::append` of a retained sub-scene for unchanged text)
  saves the encode, which is 18 ns/glyph. It does not touch the resolve or the
  GPU pipeline, because the resolver re-patches every glyph run in the appended
  encoding anyway. Not worth the retained-scene invalidation machinery.
- Animating `font-size` re-keys the outline cache every frame; snapping such an
  animation to a few discrete sizes keeps it hot. A static fractional size needs
  no such care — it is already a stable key. Either way this only protects a
  cost that is already small.
- `hint(false)` is already correct for our arbitrary transforms; turning hinting
  on would force the uniform-scale-only path in `resolve_patches`, buy nothing on
  a hidpi target, and make the cache key DPR-dependent for good measure.
- Culling off-screen text before encoding is real but is a `dom`/paint-order
  concern, not a text concern.

The one genuinely useful thing to do inside this option is **add a text
benchmark** so that any future change has a before. `crates/pulsar/benches/paint.rs`
currently benchmarks a card page with no text at all, and it measures only the
CPU encode — the half that turned out not to matter.

### 5.2 Move to `vello_hybrid` + glifo's atlas

This is where the ecosystem is going, and the compatibility facts are better
than one would expect.

**What lines up:**

- `vello_hybrid` 0.0.9 (2026-05-30) depends on **wgpu 29.0.3** — byte-identical
  to `vello` 0.9's pin. One adapter, one device, one queue; the two renderers
  can coexist during a migration instead of forcing a big-bang switch.
- Its `Scene` covers essentially everything `pulsar` uses today:
  `fill_path`, `stroke_path`, `push_clip_layer`, `push_blend_layer`,
  `push_opacity_layer`, `push_mask_layer`, `push_filter_layer`,
  `fill_blurred_rounded_rect` (our `box-shadow` fast path),
  `draw_texture_rects`, and `glyph_run`.
- It takes `peniko::FontData`, the same type Parley 0.11 hands us, so the
  text-layout side does not change at all.
- `glifo`'s atlas handles what our v1 explicitly punts on: **COLR** glyphs via
  `GlyphColr` and **bitmap** emoji, both keyed distinctly from outline entries.

**What does not, yet:**

- **The atlas is off by default.** `vello_hybrid-0.0.9/src/scene.rs:814`
  constructs its glyph backend with `atlas_cache_enabled: false`; you opt in per
  run through `GlyphRunBackend::atlas_cache(true)`. A naive port gets sparse
  strips without the cache — i.e. possibly *slower* text than today.
- Beta quality, by the maintainers' own description, and glifo is 0.1.x with
  "in development" on the tin.
- It is a second renderer to keep conformant. Every golden in
  `crates/pulsar/tests/screenshots/` is a vello-0.9 rasterization; a switch
  re-baselines all of them, and the screenshot harness deliberately has no
  per-backend golden suffix.

**Memory, concretely.** The atlas is RGBA8 (it shares `vello_common`'s
`ImageCache`/`MultiAtlasManager` with images, and carries colour glyphs), so
4 bytes/px. `AtlasConfig::default()` is **4096×4096, up to 8 pages** — that is
64 MiB for one page and 512 MiB at the cap, which is not a mobile-appropriate
default and must be overridden;
`GlyphAtlasResources::with_config(atlas_width, atlas_height, …)` takes `u16`
dimensions, so a 1024×1024 page (4 MiB) is available.

Sizing for a realistic Lynx screen: one family, five sizes {12,14,16,20,24},
~100 distinct glyphs each, ~18×18 px padded average, times glifo's
**4 subpixel buckets** = 2000 entries ≈ 650 k px ≈ 2.6 MB — comfortably one
1024×1024 page at ~62 % occupancy. Two notes on that: `SUBPIXEL_BUCKETS` is a
crate-private `const` in `glifo::atlas::key` (not configurable, and it costs a
4× entry multiplier for horizontal subpixel positioning), and
`GlyphCacheConfig::max_cached_font_size` defaults to 128 ppem, above which
glyphs bypass the atlas and draw directly — the deliberate answer to the
"caching gets inefficient as glyphs scale" problem.

Eviction is age-based like vello's (`max_entry_age: 64`,
`eviction_frequency: 64`) but with a detail worth appreciating: the maps use
`foldhash::fast::FixedState` with a fixed seed specifically so eviction order
is deterministic across processes, which keeps atlas packing reproducible.
That matters directly for us — it is what makes atlas-backed text golden-able.

### 5.3 A separate atlas text renderer alongside vello (glyphon)

[`glyphon`](https://crates.io/crates/glyphon) 0.12.0 (2026-07-09) is the
best-known standalone answer: cosmic-text for shaping, `etagere` for atlas
allocation, `lru` for eviction, a wgpu render pass for the quads. Actively
maintained, ~1.08 M downloads.

**Rule it out, for two independent reasons:**

1. **wgpu skew.** glyphon 0.12 requires `wgpu ^30.0.0`; vello 0.9 pins
   `wgpu 29.0.3`. They cannot share a device, and the workspace policy
   (`Cargo.toml`) is explicit that pulsar consumes wgpu/peniko/kurbo only
   through vello's re-exports precisely so the graph can never hold two skewed
   copies.
2. **Second shaping stack.** glyphon is built on `cosmic-text`, not Parley. We
   would be running two font-matching and shaping engines with two different
   sets of metrics, against a layout engine (`hughie`) whose closed leaf model
   is explicitly Parley. Text measured by one and painted by the other is a
   conformance bug generator.

Worth knowing about; not worth adopting.

### 5.4 Build our own atlas on top of vello 0.9

Technically possible — rasterize glyphs with `swash` (0.2.10, actively
maintained, 10 M downloads) or `vello_cpu` into a `Pixmap`, pack with
`etagere`/`guillotiere`, upload to a texture, and draw each instance as
`Scene::draw_image` on a quad.

**Don't.** It means owning subpixel-position quantization, LRU eviction and
atlas defragmentation, COLR/bitmap emoji, hinting policy and gamma — which is
precisely the pile of work glifo now exists to not duplicate, and glifo has a
funded team and a determinism story we would have to reinvent. It also fights
vello's own image cache for atlas pages. The only scenario that justifies it is
"we need atlas text on vello classic and cannot wait", and nothing in the
current roadmap suggests we are in that scenario.

## 6. Spatial locality

Worth separating from raw throughput, since it is where the two designs differ
most and it is the axis that degrades worst as pages get bigger.

**vello 0.9, per frame, per glyph instance.** The resolver gathers from one
`Arc<Encoding>` heap block per *distinct* glyph — allocations made at arbitrary
times, scattered across the heap, reached by hash lookup and pointer chase —
and copies a variable-length run of path tags and path data into the packed
stream. So the write is linear and cache-friendly; the *read* is a random walk
over as many heap blocks as there are distinct glyph/size pairs on screen. On
the GPU, binning then scatters those segments across tiles, and fine
rasterization re-reads them per covered tile. The working set is proportional
to total path-segment count, which for body text is roughly 30–60 segments per
glyph — a 1920-glyph screen is on the order of 10⁵ segments touched per frame.

**Atlas, per frame, per glyph instance.** Append a fixed-size quad (position +
atlas UV) to a linear buffer — pure sequential write, no pointer chase, no
per-glyph branch. On the GPU it is a texture fetch with 2D locality, and the
locality is better than it looks: guillotiere packs in allocation order, and
glyphs are allocated in the order text first shapes them, so the glyphs of a
word or a font-size tend to land adjacent in the atlas and share texture cache
lines. The per-frame working set collapses from "every path segment on screen"
to "every quad on screen", ~16–32 bytes each, plus a texture whose hot region
is a few hundred KB.

The 1 px `GLYPH_PADDING` glifo inserts exists for exactly this reason — the
hybrid renderer samples with `Extend::Pad`, and without transparent padding a
neighbouring glyph's pixels bleed in at strip-rasteriser overshoot.

## 7. Recommendation

**Landed with this research:** the text screenshot goldens
(`crates/pulsar/tests/screenshots.rs`, the `text_*` cases), so any renderer or
atlas change has a reviewable visual before.

**Not done, and the one thing worth doing next:** a **text** case in
`crates/pulsar/benches/paint.rs` — and more importantly a GPU-side timing
harness, since §2 shows the existing CPU-only bench measures the half that does
not matter. The §2 table came from a throwaway scaffold that was deleted; it
should be a committed benchmark before anyone acts on it.

**Explicitly do not:** restructure `paint::text`. Its shape is already what an
atlas backend wants — one `draw_glyphs`-equivalent call per Parley glyph run,
with synthesis and style carried on the run. Porting it to
`Scene::glyph_run(...).atlas_cache(true)` is a small, local change when the time
comes, and pre-emptive refactoring buys nothing.

**Next (watch, do not act):** track `vello_hybrid` for two specific signals —
the glyph atlas defaulting to *on* in `Scene::glyph_run`, and a 0.1 release or
an explicit "no longer beta". Both are cheap to check per release.

**Then (the actual fix):** port `pulsar` to `vello_hybrid`, atlas enabled, with
a deliberately small atlas page (1024×1024, 4 MiB) rather than the 64 MiB
default. Re-baseline the screenshot goldens as part of that change, not
silently. The wgpu pin matching today means this can be staged behind a feature
flag with both renderers in the graph.

**Never:** a second shaping stack (5.3), or a hand-rolled atlas (5.4).

## 8. Crate inventory

Versions and dates as of 2026-07-28, from the crates.io API.

| Crate | Version | Released | Maintained | Verdict |
|---|---|---|---|---|
| `vello` | 0.9.0 | 2026-05-15 | yes (linebender) | in use; no glyph atlas, none planned |
| `vello_hybrid` | 0.0.9 | 2026-05-30 | yes | **the target**; beta, wgpu 29.0.3 ✓, atlas opt-in |
| `vello_cpu` | 0.0.9 | 2026-05-30 | yes | CPU sibling; same glifo atlas, no GPU |
| `glifo` | 0.1.1 | 2026-05-30 | yes | the atlas itself; consumed via the renderers |
| `parley` | 0.11.0 | 2026-06-26 | yes | in use; shaping/layout, unaffected by any of this |
| `swash` | 0.2.10 | 2026-07-17 | yes | only if hand-rolling (5.4) — don't |
| `glyphon` | 0.12.0 | 2026-07-09 | yes | rejected: wgpu 30, cosmic-text |
| `cosmic-text` | 0.19.0 | 2026-04-22 | yes | rejected with glyphon |
| `wgpu_glyph`, `glyph_brush` | — | — | stale | superseded; do not consider |

## 9. Open questions

Things this note asserts from source reading and one machine's measurements,
which deserve a second data point before anything is built on them.

- **Does the atlas actually win here?** Nobody has published a
  vello-classic-vs-hybrid-with-atlas text benchmark. Our §2 numbers establish
  the cost we would be removing; they do not establish what replaces it. The
  prototype should measure, not assume.
- **Super-linearity.** Three points suggested the GPU delta grows faster than
  glyph count. Whether that is painted area, tile-count effects, or something
  in binning matters for how urgent this is on text-dense screens.
- **Discrete GPUs and mobile.** All numbers here are Apple-silicon unified
  memory. A discrete GPU pays upload cost per frame for the path stream that an
  atlas would not pay; the gap should be *wider* there, but that is a
  prediction.
- **Subpixel buckets.** glifo's fixed 4× entry multiplier is a memory cost for
  quality we may not need if we snap glyph origins to whole device pixels.
  Worth measuring against the goldens before accepting it.
