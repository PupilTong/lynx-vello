//! Document render benchmarks: style/layout/visual-order/private paint →
//! retained `DisplayList`, CPU-side only (no GPU dispatch),
//! `CodSpeed`-compatible.
//!
//! Three kinds of measurement live here, and they answer different questions.
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
//! - Every `strips_*` benchmark is the rasterizer's **CPU half** of one frame: a committed display
//!   list replayed into a `vello_gpu` scene, where every path is flattened and turned into sparse
//!   strips. That is the per-frame cost of drawing a list at all — every render pays it, a scroll
//!   or animation frame that recommits nothing included — and the `frame_*` numbers, which stop at
//!   the list, do not contain it.
//!
//! What the `frame_*` set is arranged to expose:
//!
//! - **Almost nothing is incremental yet.** [`terminus::link::Document::render`] is gated on one
//!   dirty bit, so any visual mutation rebuilds the whole paint order and re-encodes the whole
//!   scene. Every `frame_*` number here is therefore roughly the same constant, whatever the
//!   mutation touched. That is the point: the constant is the baseline the tiered-damage work has
//!   to break, and the benchmarks are shaped so that breaking it shows up as divergence between
//!   paired cases rather than as one number moving. The one mutation already off that path is a
//!   windowed scroll: the frame is baked unscrolled and a scroll tick only replays the compose
//!   program, so `frame_scroll_tick` times composition, not `render`.
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
//!   write still sets the dirty bit (`ensure_snapshot` in
//!   `crates/terminus/src/style/invalidation.rs` notes a visual mutation unconditionally), so today
//!   it costs a whole rebuild for a change nothing paints. It is the case a `Clean` damage tier
//!   should collapse to nothing.
//!
//! Two properties every `frame_*` benchmark here must keep, both easy to lose:
//!
//! 1. **The mutation must be real.** After a successful render a second `render` with no mutation
//!    returns `false` immediately without building anything. A benchmark whose step writes a value
//!    that is already there measures that early return and nothing else. Several of `terminus`'s
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

use divan::counter::ItemsCount;
use euclid::{Scale, Size2D};
use prime_radiant::TestImages;
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
use terminus::link::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use terminus::link::{Document, NodeId, PictureKey, StylesheetOrigin, Vector2D};

fn main() {
    divan::main();
}

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

const CARDS: usize = 120;

const ROW_ARGS: [usize; 2] = [64, 512];

const PARAGRAPHS: usize = 24;

const PARAGRAPH_CHARS: usize = 40;

const MARGIN: f32 = 2.0;

const CANDIDATE_OFFSETS: f32 = 256.0;

const TILES: usize = 64;

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

fn device() -> terminus::link::Device {
    terminus::link::standards_device(
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

const AUTO_ROW_CSS: &str = "
         .row { content-visibility: auto; contain-intrinsic-size: 800px 61px; }";

struct ListPage {
    dom: Document<()>,
    list: NodeId,
    visible_row: NodeId,
    offscreen_row: NodeId,
    resting_offset: f32,
    refill_offset: f32,
}

impl std::fmt::Debug for ListPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ListPage").finish_non_exhaustive()
    }
}

fn cold_list_page(rows: usize, skip_offscreen: bool) -> (Document<()>, NodeId, Vec<NodeId>) {
    assert!(rows >= 32, "a list benchmark needs more rows than it shows");
    let sheet = if skip_offscreen {
        format!("{LIST_CSS}{AUTO_ROW_CSS}")
    } else {
        LIST_CSS.to_owned()
    };
    let mut dom = page_document(&sheet);
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
    (dom, list, row_ids)
}

fn list_page(rows: usize) -> ListPage {
    list_page_with(rows, false)
}

fn auto_list_page(rows: usize) -> ListPage {
    list_page_with(rows, true)
}

fn list_page_with(rows: usize, skip_offscreen: bool) -> ListPage {
    let (mut dom, list, row_ids) = cold_list_page(rows, skip_offscreen);

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
    let mut refill_offset = resting_offset + scrollport + MARGIN;
    let last_refill_candidate = refill_offset + CANDIDATE_OFFSETS;
    while refill_offset < last_refill_candidate
        && !(clear_of_a_row_edge(refill_offset) && clear_of_a_row_edge(refill_offset + scrollport))
    {
        refill_offset += 1.0;
    }
    assert!(
        refill_offset < last_refill_candidate && refill_offset <= max,
        "no offset one window past the resting offset keeps the scrollport clear of every row edge"
    );
    assert!(
        refill_offset - resting_offset > scrollport,
        "the refill offset must leave the resting offset's encode window"
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
        refill_offset,
    }
}

struct TextKind;

impl terminus::link::CustomElement<()> for TextKind {
    fn is_lynx_text(&self) -> bool {
        true
    }
}

const PARAGRAPH_CSS: &str = "page { display: flex; flex-direction: column; position: relative;
                width: 800px; height: 600px; overflow: hidden;
                font-family: Ahem; font-size: 16px; color: #202020; }
         .paragraph { flex-shrink: 0; width: 760px; height: 24px;
                      background-color: #fbfbfd; }
         .banner { display: flex; flex-shrink: 0; width: 760px; height: 24px;
                   background-color: #2255aa; }";

fn paragraph_page(with_text: bool) -> (Document<()>, NodeId) {
    let mut dom = page_document(PARAGRAPH_CSS);
    dom.define("text", Box::new(TextKind));
    assert!(
        dom.register_fonts(terminus::link::FontBlob::from_static(AHEM)) >= 1,
        "the text cases need Ahem registered before the first layout"
    );
    let root = dom.document_element().id();
    let banner = dom.create_element("view", ());
    dom.add_class(banner, "banner");
    dom.append_child(root, banner);
    let line = "e".repeat(PARAGRAPH_CHARS);
    let mut paragraphs = Vec::with_capacity(PARAGRAPHS);
    for _ in 0..PARAGRAPHS {
        let paragraph = dom.create_element("text", ());
        dom.add_class(paragraph, "paragraph");
        dom.append_child(root, paragraph);
        if with_text {
            let text = dom.create_text_node(line.clone(), ());
            dom.append_child(paragraph, text);
        }
        paragraphs.push(paragraph);
    }
    if with_text {
        dom.layout();
        for paragraph in paragraphs {
            assert!(
                dom.text_block_size(paragraph)
                    .is_some_and(|size| size.width > 0.0),
                "every paragraph must lay its text out"
            );
        }
    }
    (dom, banner)
}

const TILE_CSS: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
         .tile { display: flex; position: absolute; width: 64px; height: 64px; }";

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

const TILE_SOURCE: &str = "app:///tile.png";

#[allow(
    clippy::cast_precision_loss,
    reason = "tile indices are small constants"
)]
fn tile_page() -> (Document<()>, Rc<TestImages>) {
    let mut dom = page_document(TILE_CSS);
    let images = Rc::new(TestImages::new());
    images.insert(TILE_SOURCE, tile_pixels(0x30));
    let root = dom.document_element().id();
    let source: std::sync::Arc<str> = std::sync::Arc::from(TILE_SOURCE);
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
        dom.set_picture(
            tile,
            Some((
                PictureKey::Url(std::sync::Arc::clone(&source)),
                Some((TILE_PIXELS, TILE_PIXELS)),
            )),
        );
    }
    (dom, images)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Staleness {
    Repaints,
    Unconstrained,
}

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
        divan::black_box(page.scene(&terminus::link::NoImages).len());
    });
}

const fn flip_color(phase: bool) -> &'static str {
    if phase {
        "rgb(20, 90, 200)"
    } else {
        "rgb(200, 90, 20)"
    }
}

const fn flip_transform(phase: bool) -> &'static str {
    if phase {
        "translateX(1px) rotate(0.5deg)"
    } else {
        "translateX(3px) rotate(1.5deg)"
    }
}

#[divan::bench(args = [24, 120])]
fn render_document(bencher: divan::Bencher<'_, '_>, cards: usize) {
    bencher
        .with_inputs(|| card_page(cards).0)
        .bench_local_values(|mut dom| {
            divan::black_box(dom.render());
            divan::black_box(dom.scene(&terminus::link::NoImages).len());
            dom
        });
}

#[divan::bench(args = ROW_ARGS)]
fn render_list_document(bencher: divan::Bencher<'_, '_>, rows: usize) {
    bencher
        .with_inputs(|| cold_list_page(rows, false).0)
        .bench_local_values(|mut dom| {
            divan::black_box(dom.render());
            divan::black_box(dom.scene(&terminus::link::NoImages).len());
            dom
        });
}

#[divan::bench(args = ROW_ARGS)]
fn render_auto_list_document(bencher: divan::Bencher<'_, '_>, rows: usize) {
    bencher
        .with_inputs(|| cold_list_page(rows, true).0)
        .bench_local_values(|mut dom| {
            divan::black_box(dom.render());
            divan::black_box(dom.scene(&terminus::link::NoImages).len());
            dom
        });
}

#[divan::bench]
fn frame_background_flip(bencher: divan::Bencher<'_, '_>) {
    let (page, card) = card_page(CARDS);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(card, "background-color", flip_color(phase));
    });
}

#[divan::bench]
fn frame_transform_flip(bencher: divan::Bencher<'_, '_>) {
    let (page, card) = card_page(CARDS);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(card, "transform", flip_transform(phase));
    });
}

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
    let mut scene = terminus::link::DisplayList::new();
    let mut phase = false;
    bencher.bench_local(move || {
        phase = !phase;
        scroll_to(&mut dom, phase);
        scene.reset();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &[],
            &|slot| Some(dom.scroll_offset(slot.node)),
            None,
        );
        divan::black_box(scene.len());
    });
}

#[divan::bench(args = ROW_ARGS)]
fn frame_visible_row_flip(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let row = page.visible_row;
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(row, "background-color", flip_color(phase));
    });
}

#[divan::bench(args = ROW_ARGS)]
fn frame_scroll_refill(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let (list, near, far) = (page.list, page.resting_offset, page.refill_offset);
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.scroll_to(list, Vector2D::new(0.0, if phase { far } else { near }));
    });
}

#[divan::bench(args = ROW_ARGS)]
fn frame_auto_scroll_refill(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = auto_list_page(rows);
    let (list, near, far) = (page.list, page.resting_offset, page.refill_offset);
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.scroll_to(list, Vector2D::new(0.0, if phase { far } else { near }));
    });
}

#[divan::bench(args = ROW_ARGS)]
fn frame_offscreen_row_flip(bencher: divan::Bencher<'_, '_>, rows: usize) {
    let page = list_page(rows);
    let row = page.offscreen_row;
    bench_frames(bencher, page.dom, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(row, "background-color", flip_color(phase));
    });
}

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

#[divan::bench]
fn frame_without_text_runs(bencher: divan::Bencher<'_, '_>) {
    let (page, banner) = paragraph_page(false);
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(banner, "background-color", flip_color(phase));
    });
}

#[divan::bench]
fn frame_with_loose_text_runs(bencher: divan::Bencher<'_, '_>) {
    let (page, banner) = loose_text_page();
    bench_frames(
        bencher.counter(ItemsCount::new(PARAGRAPHS * PARAGRAPH_CHARS)),
        page,
        Staleness::Repaints,
        move |dom, phase| {
            dom.set_inline_style_property(banner, "background-color", flip_color(phase));
        },
    );
}

fn loose_text_page() -> (Document<()>, NodeId) {
    let mut dom = page_document(&format!(
        "{PARAGRAPH_CSS}
         .paragraph.loose {{ display: flex; width: auto; align-self: flex-start; }}"
    ));
    assert!(
        dom.register_fonts(terminus::link::FontBlob::from_static(AHEM)) >= 1,
        "the text cases need Ahem registered before the first layout"
    );
    let root = dom.document_element().id();
    let banner = dom.create_element("view", ());
    dom.add_class(banner, "banner");
    dom.append_child(root, banner);
    let line = "e".repeat(PARAGRAPH_CHARS);
    let mut holders = Vec::with_capacity(PARAGRAPHS);
    for _ in 0..PARAGRAPHS {
        let holder = dom.create_element("view", ());
        dom.add_class(holder, "paragraph");
        dom.add_class(holder, "loose");
        dom.append_child(root, holder);
        let text = dom.create_text_node(line.clone(), ());
        dom.append_child(holder, text);
        holders.push(holder);
    }
    dom.layout();
    for holder in holders {
        assert_eq!(
            dom.rounded_layout(holder).map(|layout| layout.size.width),
            Some(f32::from(u8::try_from(PARAGRAPH_CHARS).expect("a short line")) * 16.0),
            "every holder must lay its loose text out"
        );
    }
    (dom, banner)
}

#[divan::bench]
fn frame_image_repaint(bencher: divan::Bencher<'_, '_>) {
    let (mut page, images) = tile_page();
    prime_radiant::render_with_images(&mut page, &images);
    let root = page.document_element().id();
    bench_frames(bencher, page, Staleness::Repaints, move |dom, phase| {
        dom.set_inline_style_property(root, "background-color", flip_color(phase));
    });
}

fn bench_strips(bencher: divan::Bencher<'_, '_>, mut dom: Document<()>) {
    dom.render();
    let list = dom.scene(&terminus::link::NoImages).clone();
    let viewport = dom.viewport_size();
    let ratio = dom.device_pixel_ratio();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a benchmark page's device size is a small pixel count"
    )]
    let (width, height) = (
        (viewport.width * ratio).round() as u16,
        (viewport.height * ratio).round() as u16,
    );
    let mut strips = terminus::link::render::strips::StripScene::new(width, height);
    assert!(strips.fill(&list) > 0, "the page draws something");
    bencher.bench_local(move || divan::black_box(strips.fill(&list)));
}

#[divan::bench]
fn strips_cards(bencher: divan::Bencher<'_, '_>) {
    bench_strips(bencher, card_page(24).0);
}

#[divan::bench]
fn strips_paragraphs(bencher: divan::Bencher<'_, '_>) {
    bench_strips(
        bencher.counter(ItemsCount::new(PARAGRAPHS * PARAGRAPH_CHARS)),
        paragraph_page(true).0,
    );
}
