# bobcat-canvas

The HTML Standard's Canvas 2D drawing model (bucket 1: the standard, no
Lynx quirk), recorded as `terminus` display lists and replayed on the GPU
by `vello_gpu`: one bitmap plus its `CanvasRenderingContext2D` state,
`Path2D`, `CanvasGradient`, `CanvasPattern`, and an image type. No
JavaScript, document or CSS parsing. The GPU side is `terminus`'s
(`terminus::link::render::canvas`, `docs/terminus-architecture.md`
"Canvas surfaces"); the binding to QuickJS and the placeholder registry
live in `bobcat-core`.

## API shape

- `Canvas`: one Rust call per context method or attribute, taking numbers,
  `&str`, keyword enums (`CompositeOp`, `LineCap`, … each with
  `from_name`/`name` for the IDL strings) or the crate's value types.
  `Canvas` is `Send`; there is no global state.
- Colors are `peniko::Color`; fonts are a typed `Font` (parley family names,
  size in px, weight, style, width). Parsing CSS strings is the caller's job.
  `kurbo` and `peniko` are `terminus`'s re-exports.
- Text calls take the caller's `&mut parley::FontContext`: the caller decides
  which fonts exist. The canvas owns its `LayoutContext`.
- `Path` is `Path2D` and shares its builder with the context's default path
  (`path::builder_methods!`).
- `Gradient` and `Pattern` are shared by reference (`Arc<Mutex<_>>`): a stop
  or pattern transform set later shows in draws recorded after it.
- `Image` is what `drawImage` and patterns sample: pixels not uploaded yet
  (`from_premultiplied`, `from_unpremultiplied`, `from_image_data`, an
  `ImageBitmap` made from `ImageData`), a texture of its own (`take_image`,
  `transferToImageBitmap`; a crop or copy of a texture), or a canvas's
  current texture (`Canvas::image`). Cloning shares it; it is `Send + Sync`
  natively. `crop` is what `createImageBitmap` makes of one; `read` waits for
  its pixels.
- `CanvasError` names the exception (`IndexSizeError`, `InvalidStateError`,
  `SyntaxError`, `TypeError`, `RangeError`).
- `commit(&CanvasFrames)` makes the bitmap what a placeholder `<canvas>`
  shows (HTML's update of an `OffscreenCanvas`'s placeholder) and clears
  `has_changes()`; it answers `terminus`'s `Committed`.

## Recording and replay

Draws are recorded into the canvas's `DisplayList` on the calling thread:
no GPU work and no lock per call. `Raster::flush` hands the list to the
canvas's `terminus` `Surface`, which replays it onto the canvas's texture.
A flush happens at every read (`get_image_data`, `image`, `take_image`,
`commit`), before `put_image_data`, at `flush()` (which `bobcat-core` calls
for each canvas drawn during a JavaScript entry), right after a draw that
samples a canvas's current texture, and once the recording passes 2 MiB
(`MAX_RECORDED_BYTES`, Chromium's bound on a canvas's recorded ops).

- A batch of `source-over` draws replays onto the texture as it is
  (`Init::Load`).
- A draw composited by another operation, a `clearRect` that does not cover
  the canvas, or a draw of the canvas itself needs the bitmap as its
  backdrop: `vello_gpu` composites a blend layer at the root into a
  transparent intermediate, and cannot sample the texture it renders into.
  What is recorded is replayed first, and the next batch starts with the
  bitmap drawn under the canvas's own texture id and replays into the
  surface's second texture (`Init::Backdrop`). A batch after a full clear
  (`reset`, a size change, a `clearRect` over the whole canvas under an
  axis-aligned transform with no clip) is replayed over a clear
  (`Init::Clear`) and needs no backdrop.
- A non-`source-over` draw is a layer with that blend mode whose region is
  the canvas rectangle; a destructive one composites over the whole clip
  region.
- The clip is a chain of canvas-space paths, kept open in the list as
  backdrop clips (`DisplayList::push_backdrop_clip`) and synced to the
  state's chain per draw, so a blend inside them reads the bitmap cut to
  the clip.
- `clearRect` is a `destination-out` layer of opaque black.
- `putImageData` premultiplies the clipped dirty rectangle and writes it into
  the texture after a flush.
- `drawImage` of a canvas: `Canvas::image` flushes the source and names its
  texture; the target flushes right after recording the draw, so later draws
  on the source are not seen. A pattern and a crop copy the texture as of
  the call. Images a recorded draw samples are kept until it is replayed.
- Shadows are a drop-shadow filter layer (`DisplayList::push_drop_shadow`)
  around the shape, composited before the shape by the state's operation
  inside the clip region. Under `source-over` each clip path is the region
  of a layer of its own around the filter layer, so the unclipped shape
  casts the shadow. Under another operation the innermost clip path is the
  blend layer's region and the paths around it stay open clips.
- Text is shaped with parley on the calling thread and recorded as glyph
  runs (`DisplayList::draw_glyphs`), with synthetic bold and oblique as
  `terminus`'s text paint records them.

## Decisions not visible from the standard

- A zero side, or a side over 8,192 px (`MAX_SURFACE_SIDE`, the longest
  texture side under the device limits `terminus` creates every device
  with), has no bitmap: draws do nothing and reads answer transparent black.
  `getImageData` over 2^28 px is `RangeError`. An image made from pixels
  with a side over 8,192 px is `InvalidStateError`.
- A readback the GPU does not answer (no device, or the Render Worker dropped
  it on wasm32) is `InvalidStateError`.
- `measureText` baselines the standard leaves to the font: em square split
  in the ratio of the first font's ascent to descent; hanging baseline at 80%
  of the ascent; ideographic-under at the descent. No `BASE` table is read.
  Ink bounds come from glyph outlines (`skrifa`) without synthetic bold or
  oblique.
- `globalAlpha` is folded into the paint, so overlapping glyphs of one text
  run blend with each other.
- A shadow under a composite operation other than `source-over` and two or
  more clip paths is cast by the shape cut to every clip path but the
  innermost.
- `Path::from_svg` with a syntax error is an empty path (the standard keeps
  the valid prefix).
- Glyphs draw as outlines: bitmap color glyphs (`sbix`/`CBDT` emoji) draw
  nothing.
- Tests use the Ahem font from `crates/seldon/tests/fixtures` and need a GPU
  adapter, as every screenshot test does; they run in CI's `runtime` shard.
