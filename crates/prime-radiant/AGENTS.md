# crates/prime-radiant

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/prime-radiant`. Paths in backticks are relative to the repository root.

Screenshot testing infrastructure, and the only crate here that exists for the
test suite rather than the product (`publish = false`, dev-dependency
everywhere). It owns RGBA `Image` + PNG codec, a port of the `pixelmatch`
algorithm Playwright compares screenshots with (squared-YIQ per-pixel distance
against `35215 * threshold²`, anti-aliasing detection,
`max_diff_pixels`/`max_diff_pixel_ratio` budgets), and `Screenshots`, the
golden store: path resolution from a name-segment list,
`PRIME_RADIANT_UPDATE_SNAPSHOTS=1` to accept, and `-expected`/`-actual`/`-diff`
PNGs written to a git-ignored `tests/artifacts/` on failure. A newly *created*
golden fails its own run so an unreviewed baseline cannot pass; an explicitly
*accepted* one does not.

The optional `render` feature adds `capture_document` (`Document::render` →
retained scene → `terminus`'s headless GPU) over the whole painted frame, `viewport
* device_pixel_ratio` device pixels: the render floor scales the scene up by
that ratio, so anything smaller is a crop. Playwright instead downsamples to
CSS pixels; the two coincide at a ratio of 1, which lynx-stack pins for
determinism and every viewport here uses. Its `TestImages` is the in-memory
`terminus::link::FrameImages` the image suites hand to a capture — the only image store in
this workspace, and deliberately a test double: it fetches, decodes and evicts
nothing, and answers URL pictures only, at generation zero. `pump_images` drives one round of the document-to-host image protocol
(every source `take_wanted_images` named and every document request
`take_document_requests` queued, then the reports back through
`apply_image_events`) and `render_with_images` loops that to quiescence.
`TestImages` parses SVG as a production host does, with the engine's
`ImageEvent::parse_document`: `insert_svg(source, &str)` reports the parsed
document through `parsed_document` (or `failed`), and `request_document`
answers a document request the same way; prime-radiant has no SVG parser
dependency of its own.
`headless` requires a usable GPU adapter and panics without one, so local and
CI runs obey the same mandatory-GPU policy. DOM-aware screenshot suites live in
`terminus`, which also keeps the direct GPU smoke tests. Goldens are not
platform-suffixed: cross-platform rasterizer noise is absorbed by tolerance,
not by per-platform baselines.
