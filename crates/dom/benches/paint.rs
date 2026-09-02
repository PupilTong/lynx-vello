//! Document render benchmarks: style/layout/visual-order/private paint →
//! retained `vello::Scene`, CPU-side only (no GPU dispatch),
//! `CodSpeed`-compatible.
//!
//! Two kinds of measurement live here, and they answer different questions.
//!
//! - `render_document` is the **cold** number: a document built by `with_inputs` and never
//!   rendered, so one iteration pays the first style flush, the first full layout, the first
//!   paint-order build, and the first scene encode together. It is what an embedder pays to put a
//!   page on screen once.
//! - Every `frame_*` benchmark is a **steady-state** number: the page is built, rendered, and both
//!   phases of its mutation are exercised before timing starts, so one iteration is one mutation
//!   plus one repaint of an already-warm document. These are the frames a running app actually
//!   produces — a scroll tick, a color change, a transform write — and none of them were observable
//!   before, because a fresh document's first render swamped them.
//!
//! What the `frame_*` set is arranged to expose:
//!
//! - **Almost nothing is incremental yet.** [`dom::Document::render`] is gated on one dirty bit, so
//!   any visual mutation rebuilds the whole paint order and re-encodes the whole scene. Every
//!   `frame_*` number here is therefore roughly the same constant, whatever the mutation touched.
//!   That is the point: the constant is the baseline the tiered-damage work has to break, and the
//!   benchmarks are shaped so that breaking it shows up as divergence between paired cases rather
//!   than as one number moving. The one mutation already off that path is a windowed scroll: the
//!   frame is baked unscrolled and a scroll tick only replays the compose program, so
//!   `frame_scroll_tick` times composition, not `render`.
//! - **Paired by reach.** `frame_visible_row_flip` and `frame_offscreen_row_flip` differ only in
//!   whether the repainted row is inside the scrollport; `frame_with_text_runs` and
//!   `frame_without_text_runs` differ only in whether the same boxes carry text. Today each pair
//!   ties. The text pair's gap is the glyph encode. The row pair's gap is the **damage tier**, not
//!   viewport culling: both cases re-encode the whole frame from the same scroll offset, so culling
//!   drops the same rows out of both and lowers them together. Reading a persistent tie there as
//!   culling having failed is the available misreading, so it is written down here.
//! - **Scaled by document size.** The list cases take a row-count argument. Their encode is already
//!   bounded by the scrollport — the walker discards items no clip chain admits — so the scene the
//!   512-row page produces is the same size as the 64-row one. What the two arguments still
//!   separate is everything *upstream* of the encode: the style flush, the layout commit, the
//!   paint-order build and the walker's own prepass all walk the whole document, and that is what
//!   the remaining ratio measures. Read the pair as the cost of rebuilding a frame the damage tiers
//!   should not be rebuilding, not as a check on culling; culling shows up in the absolute number,
//!   which fell by about three quarters at 512 rows when it landed.
//! - **`frame_inert_attribute` is the floor.** It writes a `data-` attribute no rule selects. The
//!   write still sets the dirty bit (`ensure_snapshot` in `crates/dom/src/style/invalidation.rs`
//!   notes a visual mutation unconditionally), so today it costs a whole rebuild for a change
//!   nothing paints. It is the case a `Clean` damage tier should collapse to nothing.
//!
//! Two properties every `frame_*` benchmark here must keep, both easy to lose:
//!
//! 1. **The mutation must be real.** After a successful render a second `render` with no mutation
//!    returns `false` immediately without building anything. A benchmark whose step writes a value
//!    that is already there measures that early return and nothing else. Several of `dom`'s
//!    mutators silently skip when the value does not change — `Document::add_class`/`remove_class`
//!    return early when the class is already present/absent, and
//!    `Document::set_inline_style_property` returns early when the declaration block would not
//!    change. [`Staleness::Repaints`] runs both phases before timing and asserts at least one
//!    produced a frame. (`Document::scroll_to` never invalidates a frame that carries the
//!    scroller's slot at all — which is why `frame_scroll_tick` times composition instead.)
//! 2. **The mutation must not drift.** Every step takes a `phase` and *writes from* it rather than
//!    accumulating: two colors, two transforms, two scroll offsets, two image buffers. Iteration N
//!    therefore starts where iteration 0 started. A step that scrolled one more pixel each time
//!    would walk down the list and eventually hit the clamp, where it stops invalidating
//!    altogether; a step that grew a declaration block would report a rising cost that is the
//!    benchmark's own doing. The two phases are also chosen to be structurally identical — both
//!    colors opaque, both transforms non-`none`, both images the same size — so the two frames
//!    encode the same draw count and the mean is one number rather than an average of two shapes.

use std::rc::Rc;
use std::sync::Arc;

use divan::counter::ItemsCount;
use dom::layout::{NaturalSize, Size};
use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{Document, NodeId, StylesheetOrigin, Vector2D};
use euclid::{Scale, Size2D};
use flashbulb::TestImages;
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};

fn main() {
    divan::main();
}

/// Ahem: one filled em box per glyph, no kerning, no ligatures. Registering it
/// pins the text cases to one shaping outcome on every runner instead of to
/// whatever the host's fallback font happens to be.
const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

/// Elements on the card page the single-element cases mutate. Matches
/// `benches/animation.rs` so the two files price the same document.
const CARDS: usize = 120;

/// Rows in the list page. The two arguments differ by 8x with the same
/// scrollport, so a cost that tracks the document separates from one that
/// tracks the visible rows.
const ROW_ARGS: [usize; 2] = [64, 512];

/// Text-carrying boxes on the paragraph page.
const PARAGRAPHS: usize = 24;

/// Characters per paragraph. Ahem maps every ASCII character to exactly one
/// glyph, so `PARAGRAPHS * PARAGRAPH_CHARS` is the glyph count the encode pass
/// walks.
const PARAGRAPH_CHARS: usize = 40;

/// How far a scrollport edge is kept from any row boundary, in CSS px. One
/// scroll step is one pixel, so two is enough for both phases to clear.
const MARGIN: f32 = 2.0;

/// How many whole-pixel offsets past the list's midpoint [`list_page`] will try
/// before giving up on finding one that clears every row boundary. A row is
/// far shorter than this, so a clear offset always exists within it.
const CANDIDATE_OFFSETS: f32 = 256.0;

/// Replaced-content tiles on the image page.
const TILES: usize = 64;

/// Edge of a decoded tile, in pixels.
const TILE_PIXELS: u32 = 32;

#[derive(Debug)]
struct BenchFontMetricsProvider;

impl FontMetricsProvider for BenchFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}

fn device() -> dom::Device {
    dom::standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(800.0, 600.0),
        Size2D::<f32, DevicePixel>::new(800.0, 600.0),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

// ---------------------------------------------------------------------------
// Page fixtures
// ---------------------------------------------------------------------------

/// `dom` ships no user-agent sheet — [`Document::new`] installs the document
/// element and nothing else, and the Lynx cascade (`page`/`view` box sizing,
/// the `display` default, the `overflow` default) lives in `bobcat-core`,
/// above this crate. Every fixture here therefore states its own geometry.
///
/// Two defaults still come from the Stylo fork rather than from any sheet and
/// are relied on below: `display` initializes to `flex`, and `overflow`
/// initializes to `visible`. A box that must clip says so.
fn page_document(css: &str) -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
    dom.add_stylesheet(css, StylesheetOrigin::Author);
    dom
}

const CARD_CSS: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
         .card { display: flex; position: absolute; width: 180px; height: 80px;
                 background-color: #f6f6f8; border: 2px solid #cccccc;
                 border-radius: 10px; box-shadow: 0px 2px 6px rgba(0,0,0,0.25); }
         .fade { opacity: 0.85; }
         .clip { overflow: hidden; }
         .chip { display: flex; width: 60px; height: 20px;
                 background-color: #3366ff; border-radius: 10px; }";

/// The original render fixture, plus the id of the last card.
///
/// The tree and the sheet are unchanged from when `render_document` was the
/// only benchmark in this file, so that benchmark's history stays comparable.
/// Every mutation the `frame_*` cases apply to this page is an inline style or
/// an attribute, never a sheet edit, for the same reason.
fn card_page(cards: usize) -> (Document<()>, NodeId) {
    let mut dom = page_document(CARD_CSS);
    let root = dom.document_element().id();
    let mut last = root;
    for index in 0..cards {
        let card = dom.create_element("view", ());
        dom.add_class(card, "card");
        if index % 3 == 0 {
            dom.add_class(card, "fade");
        }
        if index % 2 == 0 {
            dom.add_class(card, "clip");
        }
        dom.append_child(root, card);
        let chip = dom.create_element("view", ());
        dom.add_class(chip, "chip");
        dom.append_child(card, chip);
        last = card;
    }
    (dom, last)
}

const LIST_CSS: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
         .list { display: flex; flex-direction: column; overflow: scroll;
                 width: 800px; height: 600px; }
         .row { display: flex; flex-shrink: 0; width: 800px; height: 60px;
                background-color: #eef1f5; border-bottom: 1px solid #d8dde3; }
         .cell { display: flex; width: 120px; height: 40px;
                 background-color: #ccd4dd; border-radius: 6px; }";

/// A tall scrolling list, scrolled to its midpoint so most rows sit outside the
/// scrollport.
///
/// `flex-shrink: 0` is load-bearing: without it the column would compress every
/// row into the 600px scrollport, the scrolling area would equal the
/// scrollport, and `scroll_to` would clamp every offset to zero — the list
/// would stop being a list.
struct ListPage {
    dom: Document<()>,
    /// The scroll container.
    list: NodeId,
    /// A row inside the scrollport at the resting offset.
    visible_row: NodeId,
    /// The first row, far above the scrollport at the resting offset.
    offscreen_row: NodeId,
    /// The offset the list rests at. Both scroll phases are within a pixel of
    /// it and neither clamps.
    resting_offset: f32,
}

impl std::fmt::Debug for ListPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ListPage").finish_non_exhaustive()
    }
}

fn list_page(rows: usize) -> ListPage {
    assert!(rows >= 32, "a list benchmark needs more rows than it shows");
    let mut dom = page_document(LIST_CSS);
    let root = dom.document_element().id();
    let list = dom.create_element("view", ());
    dom.add_class(list, "list");
    dom.append_child(root, list);

    let mut row_ids = Vec::with_capacity(rows);
    for _ in 0..rows {
        let row = dom.create_element("view", ());
        dom.add_class(row, "row");
        dom.append_child(list, row);
        let cell = dom.create_element("view", ());
        dom.add_class(cell, "cell");
        dom.append_child(row, cell);
        row_ids.push(row);
    }

    dom.layout();
    let scroll_box = dom
        .scroll_box(list)
        .expect("`overflow: scroll` on a sized box makes it a scroll container");
    let scrollport = scroll_box.scrollport.height;
    let max = scroll_box.max_offset().y;
    assert!(
        max > MARGIN,
        "the rows must overflow the scrollport by more than one scroll step, got {max}"
    );

    // Where the rows actually are is read back from the laid-out boxes rather
    // than derived from the sheet: a row's outer height depends on
    // `box-sizing` and on the border the sheet gives it, and a benchmark that
    // guessed wrong would silently pair two rows on the same side of the clip.
    let edges: Vec<f32> = row_ids
        .iter()
        .flat_map(|row| {
            let layout = dom
                .rounded_layout(*row)
                .expect("a laid-out row has a rounded layout");
            [layout.location.y, layout.location.y + layout.size.height]
        })
        .collect();
    let clear_of_a_row_edge = |edge: f32| {
        edges
            .iter()
            .all(|boundary| (boundary - edge).abs() >= MARGIN)
    };

    // Both scroll phases must show the same rows. Nothing culls today, so
    // either offset paints every row regardless; once something does, an
    // offset sitting on a row boundary would pull one row in on one phase and
    // drop it on the other, and the reported mean would be an average of two
    // different frames rather than one frame's cost. Both scrollport edges are
    // therefore parked at least `MARGIN` away from every row boundary.
    let mut resting_offset = (max * 0.5).floor();
    let last_candidate = resting_offset + CANDIDATE_OFFSETS;
    while resting_offset < last_candidate
        && !(clear_of_a_row_edge(resting_offset)
            && clear_of_a_row_edge(resting_offset + 1.0)
            && clear_of_a_row_edge(resting_offset + scrollport)
            && clear_of_a_row_edge(resting_offset + 1.0 + scrollport))
    {
        resting_offset += 1.0;
    }
    assert!(
        resting_offset < last_candidate && resting_offset + 1.0 <= max,
        "no offset near the list's midpoint keeps both scroll phases showing the same rows"
    );
    dom.scroll_to(list, Vector2D::new(0.0, resting_offset));

    let row_extent = |dom: &Document<()>, row: NodeId| {
        let layout = dom
            .rounded_layout(row)
            .expect("a laid-out row has a rounded layout");
        (layout.location.y, layout.location.y + layout.size.height)
    };
    let visible_row = *row_ids
        .iter()
        .find(|row| {
            let (top, bottom) = row_extent(&dom, **row);
            top >= resting_offset && bottom <= resting_offset + scrollport
        })
        .expect("the resting offset must show at least one whole row");
    let offscreen_row = row_ids[0];
    let (_, offscreen_bottom) = row_extent(&dom, offscreen_row);
    assert!(
        offscreen_bottom <= resting_offset,
        "the offscreen row must sit entirely above the scrollport at rest"
    );

    ListPage {
        dom,
        list,
        visible_row,
        offscreen_row,
        resting_offset,
    }
}

const PARAGRAPH_CSS: &str = "page { display: flex; flex-direction: column; position: relative;
                width: 800px; height: 600px; overflow: hidden;
                font-family: Ahem; font-size: 16px; color: #202020; }
         .paragraph { display: flex; flex-shrink: 0; width: 760px; height: 24px;
                      background-color: #fbfbfd; }
         .banner { display: flex; flex-shrink: 0; width: 760px; height: 24px;
                   background-color: #2255aa; }";

/// `PARAGRAPHS` identical boxes, each carrying a text run when `with_text`.
///
/// The two variants share their box tree, their sheet, and their mutation, so
/// the difference between the benchmarks over them is the glyph encode and
/// nothing else. The `banner` is the box the mutation touches: the text is
/// never rewritten, because reshaping is a different cost than encoding and
/// mixing them would make neither readable.
fn paragraph_page(with_text: bool) -> (Document<()>, NodeId) {
    let mut dom = page_document(PARAGRAPH_CSS);
    assert!(
        dom.register_fonts(dom::FontBlob::from_static(AHEM)) >= 1,
        "the text cases need Ahem registered before the first layout"
    );
    let root = dom.document_element().id();
    let banner = dom.create_element("view", ());
    dom.add_class(banner, "banner");
    dom.append_child(root, banner);
    let line = "e".repeat(PARAGRAPH_CHARS);
    for _ in 0..PARAGRAPHS {
        let paragraph = dom.create_element("view", ());
        dom.add_class(paragraph, "paragraph");
        dom.append_child(root, paragraph);
        if with_text {
            let text = dom.create_text_node(line.clone(), ());
            dom.append_child(paragraph, text);
        }
    }
    (dom, banner)
}

const TILE_CSS: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
         .tile { display: flex; position: absolute; width: 64px; height: 64px; }";

/// Decoded pixels for one tile: a flat color, so both phases of the swap are
/// the same length and the same dimensions and only the bytes differ.
fn tile_pixels(shade: u8) -> ImageData {
    let side = TILE_PIXELS as usize;
    let mut rgba = Vec::with_capacity(side * side * 4);
    for _ in 0..(side * side) {
        rgba.extend_from_slice(&[shade, 0x40, 0xff - shade, 0xff]);
    }
    ImageData {
        data: Blob::from(rgba),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: TILE_PIXELS,
        height: TILE_PIXELS,
    }
}

/// The source every tile in [`tile_page`] draws.
const TILE_SOURCE: &str = "app:///tile.png";

/// A grid of replaced-content tiles, all inside the viewport, plus the store
/// holding their pixels.
///
/// Every tile draws the same source, which is what a page of identical
/// replaced boxes does: one entry in the registry, one resolve per commit,
/// and the same reference-counted buffer behind every draw.
#[allow(
    clippy::cast_precision_loss,
    reason = "tile indices are small constants"
)]
fn tile_page() -> (Document<()>, Rc<TestImages>) {
    let mut dom = page_document(TILE_CSS);
    let images = Rc::new(TestImages::new());
    images.insert(TILE_SOURCE, tile_pixels(0x30));
    let root = dom.document_element().id();
    let natural = NaturalSize::from_size(Size::new(TILE_PIXELS as f32, TILE_PIXELS as f32));
    for index in 0..TILES {
        let tile = dom.create_element("view", ());
        dom.add_class(tile, "tile");
        dom.set_inline_style(
            tile,
            &format!(
                "left: {}px; top: {}px",
                (index % 10) * 64,
                (index / 10) * 64
            ),
        );
        dom.append_child(root, tile);
        dom.set_natural_size(tile, natural);
        dom.set_image_source(tile, Some(TILE_SOURCE));
    }
    (dom, images)
}

// ---------------------------------------------------------------------------
// Frame harness
// ---------------------------------------------------------------------------

/// Whether the step under test is required to invalidate the retained scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Staleness {
    /// At least one phase must make `Document::render` rebuild. Asserted
    /// before timing, because a step that writes a value already in place is
    /// a no-op the render path exits from immediately, and the benchmark
    /// would then measure that exit.
    ///
    /// One phase, not both, because a phase that legitimately stops
    /// invalidating is a win the assertion must not report as a failure: a
    /// damage tier that classifies one of two alternating writes clean is
    /// exactly the outcome this file exists to measure. Two clean phases is
    /// the case that cannot be a win — the benchmark would be timing an early
    /// return.
    Repaints,
    /// The step's effect on the retained scene is itself the subject, so
    /// nothing is asserted. `frame_inert_attribute` rebuilds today and should
    /// stop once a damage tier can classify it clean; pinning today's answer
    /// would turn that improvement into a benchmark failure.
    Unconstrained,
}

/// Times one steady-state frame: apply the step for this phase, then produce
/// the scene.
///
/// Everything outside the timed closure runs once per benchmark, not once per
/// iteration: building the page, its first layout, its first paint order, its
/// first scene encode, the Parley font collection the text cases force open,
/// and both phases of the step. What is left inside is one mutation and one
/// repaint of a warm document.
///
/// This deliberately does not use `Bencher::with_inputs`. `with_inputs` builds
/// a fresh input per iteration, and a fresh document has never rendered, so
/// every iteration would pay a first render again — which is `render_document`,
/// not a frame.
fn bench_frames(
    bencher: divan::Bencher<'_, '_>,
    page: Document<()>,
    staleness: Staleness,
    mut step: impl FnMut(&mut Document<()>, bool),
) {
    let mut page = page;
    page.render();
    assert!(
        !page.needs_render(),
        "a rendered page must start the frame loop clean"
    );
    let mut repaints = 0_u32;
    for phase in [true, false] {
        step(&mut page, phase);
        repaints += u32::from(page.render());
    }
    assert!(
        repaints > 0 || staleness == Staleness::Unconstrained,
        "neither phase of this step made the retained scene stale, so the \
         benchmark would time `Document::render`'s early return"
    );

    let mut phase = false;
    bencher.bench_local(move || {
        phase = !phase;
        step(&mut page, phase);
        divan::black_box(page.render());
        divan::black_box(page.scene(&dom::NoImages).encoding().draw_tags.len());
    });
}

/// Two opaque solids. Both paint one background draw, so the two phases encode
/// the same scene shape; a transparent phase would skip the draw entirely and
/// the reported mean would be an average of two different frames.
const fn flip_color(phase: bool) -> &'static str {
    if phase {
        "rgb(20, 90, 200)"
    } else {
        "rgb(200, 90, 20)"
    }
}

/// Two non-singular, non-`none` transforms. `none` on one phase would drop the
/// element's stacking context and change the paint order's shape between
/// phases.
const fn flip_transform(phase: bool) -> &'static str {
    if phase {
        "translateX(1px) rotate(0.5deg)"
    } else {
        "translateX(3px) rotate(1.5deg)"
    }
}

// ---------------------------------------------------------------------------
// Cold render — unchanged, and named as it was, so its history carries over
// ---------------------------------------------------------------------------

/// The first render of a freshly built document: style flush, full layout, full
/// paint-order build, full scene encode.
#[divan::bench(args = [24, 120])]
fn render_document(bencher: divan::Bencher<'_, '_>, cards: usize) {
    bencher
        .with_inputs(|| card_page(cards).0)
        .bench_local_values(|mut dom| {
            divan::black_box(dom.render());
            divan::black_box(dom.scene(&dom::NoImages).encoding().draw_tags.len());
            dom
        });
}

// ---------------------------------------------------------------------------
// Single-element frames on the card page
// ---------------------------------------------------------------------------

/// One element's `background-color` changes. Paint-only: nothing moves, no box
/// is re-measured, and no stacking context appears or disappears — so this is
/// the smallest visual change the engine can be asked to show, against a
/// document of `CARDS` cards it has to rebuild anyway.
#[divan::bench]
fn frame_background_flip(bencher: divan::Bencher<'_, '_>) {
    let (page, card) = card_page(CARDS);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(card, "background-color", flip_color(phase));
    });
}

/// One element's `transform` changes. Still paint-only — a transform cannot
/// move a box — but unlike a color it changes the matrix every descendant item
/// is composed against, which is the mutation a compositor-driven animation
/// produces every frame.
#[divan::bench]
fn frame_transform_flip(bencher: divan::Bencher<'_, '_>) {
    let (page, card) = card_page(CARDS);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(card, "transform", flip_transform(phase));
    });
}

/// A `data-` attribute no rule selects. Nothing about the page's appearance
/// changes, and today the whole scene is rebuilt anyway: taking a snapshot
/// notes a visual mutation without asking whether any rule cares. This is the
/// frame a `Clean` damage tier has to make free.
#[divan::bench]
fn frame_inert_attribute(bencher: divan::Bencher<'_, '_>) {
    let (page, card) = card_page(CARDS);
    bench_frames(
        bencher,
        page,
        Staleness::Unconstrained,
        move |dom, phase| {
            dom.set_attribute(card, "data-bench-tick", if phase { "1" } else { "0" });
        },
    );
}

// ---------------------------------------------------------------------------
// List frames — scrolling, and repaints inside and outside the scrollport
// ---------------------------------------------------------------------------

/// One pixel of scroll on a list whose rows mostly sit outside the scrollport.
///
/// The offset alternates between two values a pixel apart around the list's
/// midpoint instead of advancing. Advancing would eventually leave the
/// committed encode window, where the production path recommits; it would
/// also change how many rows are inside the scrollport as it went.
///
/// Deliberately not [`bench_frames`]: a windowed scroll no longer touches
/// the retained frame at all — the frame is baked unscrolled, and a scroll
/// tick's whole cost is replaying its compose program at the live offsets,
/// which is exactly what the presenting side does per frame. The loop times
/// that path: move the document's offset, recompose into a reused scene
/// buffer. The guard is the inverse of `Staleness::Repaints`: a scroll that
/// starts invalidating the retained frame again is a regression this
/// benchmark must fail on, because it would silently go back to timing a
/// full rebuild.
#[divan::bench(args = ROW_ARGS)]
fn frame_scroll_tick(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let mut page = list_page(rows);
    let list = page.list;
    let resting = page.resting_offset;
    let scroll_to = move |dom: &mut Document<()>, phase: bool| {
        let offset = if phase { resting + 1.0 } else { resting };
        dom.scroll_to(list, Vector2D::new(0.0, offset))
    };
    assert_ne!(
        scroll_to(&mut page.dom, true),
        scroll_to(&mut page.dom, false),
        "both scroll phases clamped to the same offset, so the step scrolls nothing",
    );

    let mut dom = page.dom;
    dom.render();
    scroll_to(&mut dom, true);
    assert!(
        !dom.needs_render(),
        "a windowed scroll must stay compose-only; invalidating the retained \
         frame here means this benchmark is back to timing a full rebuild"
    );
    let frame = dom
        .committed_frame()
        .expect("render always leaves a committed frame retained");
    assert!(
        frame.slot_of(list).is_some(),
        "the committed frame must carry the list's scroll slot"
    );
    let mut scene = dom::vello::Scene::new();
    let mut phase = false;
    bencher.bench_local(move || {
        phase = !phase;
        scroll_to(&mut dom, phase);
        scene.reset();
        frame.compose_into(
            &mut scene,
            &[],
            &|slot| Some(dom.scroll_offset(slot.node)),
            None,
        );
        divan::black_box(scene.encoding().draw_tags.len());
    });
}

/// The same scroll tick through the layered path: the frame's plan
/// composed, each retained plane one textured draw. This is what a GPU
/// target encodes per frame for a layered frame — the raw steps plus the
/// plane draws — while `frame_scroll_tick` above replays the scroller
/// content itself, which is the fallback path's cost. The plane images are
/// stand-ins with the plan's dimensions; nothing here renders, and the
/// encode does not read pixels.
#[divan::bench(args = ROW_ARGS)]
fn frame_scroll_composite(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let list = page.list;
    let resting = page.resting_offset;
    let scroll_to = move |dom: &mut Document<()>, phase: bool| {
        let offset = if phase { resting + 1.0 } else { resting };
        dom.scroll_to(list, Vector2D::new(0.0, offset))
    };
    let mut dom = page.dom;
    dom.render();
    let frame = dom
        .committed_frame()
        .expect("render always leaves a committed frame retained");
    let plan = frame
        .composite_plan()
        .expect("the list frame carries scroller content, so it layers");
    let planes: Vec<ImageData> = (0..plan.plane_count())
        .map(|index| {
            let (width, height) = plan.plane_size(index);
            ImageData {
                data: Blob::new(Arc::new([])),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width,
                height,
            }
        })
        .collect();
    let mut scene = dom::vello::Scene::new();
    let mut phase = false;
    bencher.bench_local(move || {
        phase = !phase;
        scroll_to(&mut dom, phase);
        scene.reset();
        frame.composite_into(
            &mut scene,
            &planes,
            &[],
            &|slot| Some(dom.scroll_offset(slot.node)),
            None,
        );
        divan::black_box(scene.encoding().draw_tags.len());
    });
}

/// A `background-color` flip on a row the scrollport shows. The control for
/// `frame_offscreen_row_flip`: this frame has to repaint something visible, so
/// it is the cost a correct engine cannot avoid.
#[divan::bench(args = ROW_ARGS)]
fn frame_visible_row_flip(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let row = page.visible_row;
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(row, "background-color", flip_color(phase));
    });
}

/// The same flip on a row far above the scrollport, clipped away by the list.
/// Nothing the user can see changes, and today it costs what the visible flip
/// costs, on both row counts.
///
/// Viewport culling does not separate this from its control — both frames are
/// re-encoded from the same scroll offset, so culling drops the same rows out
/// of both. What separates them is a damage tier that can decide the repaint
/// is entirely clipped away and skip the frame.
#[divan::bench(args = ROW_ARGS)]
fn frame_offscreen_row_flip(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let row = page.offscreen_row;
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(row, "background-color", flip_color(phase));
    });
}

// ---------------------------------------------------------------------------
// Text frames — the glyph encode, isolated by a paired empty page
// ---------------------------------------------------------------------------

/// A frame of a page carrying `PARAGRAPHS * PARAGRAPH_CHARS` glyphs. The
/// mutation touches a box that carries no text, so no run is reshaped and no
/// line is re-broken: what repeats every frame is the encode of retained Parley
/// layouts into glyph runs.
#[divan::bench]
fn frame_with_text_runs(bencher: divan::Bencher<'_, '_>) {
    let (page, banner) = paragraph_page(true);
    bench_frames(
        bencher.counter(ItemsCount::new(PARAGRAPHS * PARAGRAPH_CHARS)),
        page,
        Staleness::Repaints,
        move |dom, phase| {
            dom.set_inline_style_property(banner, "background-color", flip_color(phase));
        },
    );
}

/// The same page, the same boxes, the same mutation, with the text nodes left
/// out. Subtracting this from `frame_with_text_runs` leaves the glyph encode.
#[divan::bench]
fn frame_without_text_runs(bencher: divan::Bencher<'_, '_>) {
    let (page, banner) = paragraph_page(false);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(banner, "background-color", flip_color(phase));
    });
}

// ---------------------------------------------------------------------------
// Image frames
// ---------------------------------------------------------------------------

/// A page of `TILES` replaced boxes, all drawing one already-loaded image,
/// repainting.
///
/// This is the cost that actually recurs. An image *arriving* cannot be
/// benchmarked in two phases any more: one URL has one content, so a load
/// report lands exactly once per document and the second phase would
/// invalidate nothing. What a page pays over and over is rebuilding its image
/// draws — one `ImageDraw` per tile, and one program op per draw — which is
/// what this measures, against a mutation that touches no image at all.
#[divan::bench]
fn frame_image_repaint(bencher: divan::Bencher<'_, '_>) {
    let (mut page, images) = tile_page();
    // Load once, before timing: from here the registry is `Ready` and every
    // commit draws real pixels.
    flashbulb::render_with_images(&mut page, &images);
    let root = page.document_element().id();
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(root, "background-color", flip_color(phase));
    });
}
