//! css-contain-3 container-unit benchmarks: what `cqw`/`cqh` over a
//! `container-type` query container cost inside one `Document::layout` call.
//!
//! A query container's content box is a cascade input that only layout can
//! produce. For a `container-type: size` box the layout run settles that
//! itself — its size is contained in both axes, so the host computes it at the
//! entry of the box's own committing run, publishes it, recascades the
//! `cqw`/`cqh` readers under it and lays the subtree out once, at the size it
//! keeps. `Document::layout`'s loop — lay out, mark the readers under every
//! container whose recorded box moved, lay out again, up to
//! `CONTAINER_PASSES` times — is the fallback for what that cannot predict
//! (`inline-size` containers and `display: -lynx-text` ones). Both are gated
//! on a sticky document bit set when a cascaded style resolved a
//! container-relative unit, so a page with query containers and no `cqw`/`cqh`
//! pays one emptiness test and one bit test for them.
//!
//! Every case lays out the same page shape — `GROUPS` groups of
//! `LEAVES_PER_GROUP` leaves, sized by constants — so an `args` value names
//! only *where the containers and the units are*, never how many elements
//! there are. The counter is the leaf count, so the reported throughput is
//! per element under a container.
//!
//! - **`first_layout`** — one `layout()` on a document whose styles were never flushed.
//!   `px_leaves_in_containers` is the baseline: the containers are there, nothing resolves a
//!   container unit, the gate stays shut and the call defers nothing. `cqw_leaves_in_containers` is
//!   the interleave in full: the flush resolves every leaf against the viewport fallback, each
//!   container then publishes a width narrower than the viewport at the entry of its own run, and
//!   every leaf under it is re-cascaded and laid out once — at 30px, rather than at 40px and then
//!   at 30px. `cqw_leaves_root_container` makes the page itself the only container, whose content
//!   box equals the viewport, so the recascade produces the styles the leaves already had — the
//!   recascade half with the relayout half removed. `mixed_leaves_in_containers` is the same page
//!   with one `cqw` leaf per group among 99 px ones: the container moves and 1 % of its subtree
//!   reads it, which is what separates re-cascading a resized container's whole subtree from
//!   re-cascading the elements that resolved a unit. `vw_leaves` is the ordinary viewport-unit path
//!   for reference: the same 40px leaf width reached without any container machinery. It carries no
//!   containers at all, so it is not a unit-for-unit comparison against `px_leaves_in_containers` —
//!   a group that is not a size container sizes to its contents instead of sizing as if empty — and
//!   only the `cqw` and `mixed` cases are read against that baseline.
//! - **`container_resize`** — every group's width is set through an inline style on a laid-out
//!   document, then `layout()` once. `px_leaves` resizes the same containers with the gate shut, so
//!   it is the cost of the resize itself; `cqw_leaves` is that plus the recascade of every leaf
//!   under a moved container and the relayout the new widths force, both of which now happen inside
//!   the one run; `mixed_leaves` is the same resize where one leaf per container reads it.
//! - **`viewport_resize`** — `set_viewport` then `layout()` on a laid-out document. The groups are
//!   a fixed `300px` wide, so no container moves and the loop never runs a second pass in either
//!   shape. What is left is what a container unit costs a whole-document recascade that would have
//!   happened anyway: the walk up the ancestor chain per resolution, and the style-sharing cache
//!   refusing to share a `USES_CONTAINER_UNITS` style across two different parents.
//!
//! Re-cascading only the elements whose style carries
//! `USES_CONTAINER_UNITS`, rather than a resized container's whole subtree,
//! is measured by the two `mixed` cases against their `px` baselines. It is
//! *not* measurable in the all-`cqw` shapes: every element under the
//! container is a reader there, so the targeted walk marks exactly the set
//! the subtree mark did, and what those cases report is what finding them
//! costs — one subtree walk, and a selector rematch per reader in place of a
//! bare recascade. The interleave is what the gap between
//! `first_layout/cqw_leaves_in_containers` and
//! `first_layout/px_leaves_in_containers` measures, and it roughly halves it
//! (1.54 ms of container term to 0.86 ms, paired on one machine). What is left
//! of that gap is the cascade a reader pays twice — once in the pass's own
//! flush, against the viewport fallback, and once against its container — which
//! only leaving a container's subtree out of that first flush could remove.

use divan::black_box;
use divan::counter::ItemsCount;
use dom::{Document, NodeId, StylesheetOrigin};
use euclid::{Scale, Size2D};
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

/// Query containers on the page, and elements under each. Fixed, so the
/// `args` axis is the page's shape and not its size.
const GROUPS: usize = 30;
const LEAVES_PER_GROUP: usize = 100;

/// What the counter reports throughput over: the elements that resolve a
/// container unit in the cases that have any.
const LEAVES: usize = GROUPS * LEAVES_PER_GROUP;

/// The viewport `cqw`'s fallback resolves against, and the size
/// `cqw_leaves_root_container` deliberately gives its one container.
const VIEWPORT_WIDTH: f32 = 400.0;
const VIEWPORT_HEIGHT: f32 = 600.0;

/// The narrower viewport `viewport_resize` moves to. No box on the page is
/// sized against the viewport in the `cq` shapes, so the resize buys a
/// document-wide recascade and nothing else.
const RESIZED_VIEWPORT_WIDTH: f32 = 380.0;

/// The width `container_resize` gives every group, different from the
/// authored `300px` in both axes of the comparison.
const RESIZED_CONTAINER: &str = "width: 250px";

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
        Size2D::<f32, CSSPixel>::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
        Size2D::<f32, DevicePixel>::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

/// The groups are size query containers and the leaves are 10px wide: no
/// style on the page resolves a container unit, so the gate on the recascade
/// loop never opens.
const PX_LEAVES_IN_CONTAINERS: &str = "
page { display: flex; flex-direction: column; width: 400px; height: 600px; }
.group { display: flex; flex-direction: column; container-type: size; width: 300px; }
.leaf { display: flex; height: 2px; width: 10px; }
";

/// The same containers with `10cqw` leaves. The first pass resolves them
/// against the 400px viewport fallback and the second against the 300px
/// container, so a leaf's width moves from 40px to 30px: the loop runs a
/// recascade *and* a relayout.
const CQW_LEAVES_IN_CONTAINERS: &str = "
page { display: flex; flex-direction: column; width: 400px; height: 600px; }
.group { display: flex; flex-direction: column; container-type: size; width: 300px; }
.leaf { display: flex; height: 2px; width: 10cqw; }
";

/// The page is the only query container and its content box is the viewport,
/// so the second pass re-cascades every leaf into the width it already had.
/// The relayout half of the loop is removed and the recascade half is not.
const CQW_LEAVES_ROOT_CONTAINER: &str = "
page { display: flex; flex-direction: column; container-type: size;
       width: 400px; height: 600px; }
.group { display: flex; flex-direction: column; width: 300px; }
.leaf { display: flex; height: 2px; width: 10cqw; }
";

/// The same containers with one `10cqw` leaf per group among 99 px ones —
/// the shape a page that uses the feature sparingly has, and the one the
/// targeted recascade is for. Every element under the container reads it in
/// the two all-`cqw` shapes above, so no marking rule can visit fewer of
/// them; here 1 % of the subtree does.
const MIXED_LEAVES_IN_CONTAINERS: &str = "
page { display: flex; flex-direction: column; width: 400px; height: 600px; }
.group { display: flex; flex-direction: column; container-type: size; width: 300px; }
.leaf { display: flex; height: 2px; width: 10px; }
.reader { display: flex; height: 2px; width: 10cqw; }
";

/// No containers at all, and leaves 10% of the viewport wide — the same 40px
/// by the path that has always existed.
const VW_LEAVES: &str = "
page { display: flex; flex-direction: column; width: 400px; height: 600px; }
.group { display: flex; flex-direction: column; width: 300px; }
.leaf { display: flex; height: 2px; width: 10vw; }
";

/// The five shapes `first_layout` compares.
const FIRST_LAYOUT_SHAPES: [&str; 5] = [
    "px_leaves_in_containers",
    "cqw_leaves_in_containers",
    "mixed_leaves_in_containers",
    "cqw_leaves_root_container",
    "vw_leaves",
];

/// The three a resize compares: the same page, the leaf unit apart.
const RESIZE_SHAPES: [&str; 3] = ["px_leaves", "cqw_leaves", "mixed_leaves"];

fn shape_css(shape: &str) -> &'static str {
    match shape {
        "px_leaves_in_containers" | "px_leaves" => PX_LEAVES_IN_CONTAINERS,
        "cqw_leaves_in_containers" | "cqw_leaves" => CQW_LEAVES_IN_CONTAINERS,
        "mixed_leaves_in_containers" | "mixed_leaves" => MIXED_LEAVES_IN_CONTAINERS,
        "cqw_leaves_root_container" => CQW_LEAVES_ROOT_CONTAINER,
        "vw_leaves" => VW_LEAVES,
        other => panic!("no page shape named {other}"),
    }
}

/// Whether the shape gives each group's last leaf the class that reads the
/// container. The settled-width check runs on that leaf, so a mixed shape is
/// checked against the reader's value rather than a px one.
fn has_reader_leaf(shape: &str) -> bool {
    shape.starts_with("mixed")
}

/// The width a leaf must end one settled `layout()` at — what makes each
/// shape the case it is named for, checked where it costs nothing.
fn settled_leaf_width(shape: &str) -> f32 {
    match shape {
        "px_leaves_in_containers" | "px_leaves" => 10.0,
        // 10cqw of the 300px container, after the pass that resolved it
        // against the 400px viewport fallback.
        "cqw_leaves_in_containers"
        | "cqw_leaves"
        | "mixed_leaves_in_containers"
        | "mixed_leaves" => 30.0,
        // 10cqw of a container whose content box is the viewport, and 10vw of
        // the same viewport.
        "cqw_leaves_root_container" | "vw_leaves" => 40.0,
        other => panic!("no page shape named {other}"),
    }
}

/// One benchmark's document: the groups a resize writes to, and one leaf the
/// builder checks the shape against.
struct Page {
    doc: Document<()>,
    groups: Vec<NodeId>,
    leaf: NodeId,
}

/// A built but never flushed document.
fn page(shape: &str) -> Page {
    let mut doc = Document::new(device(), "page", ());
    doc.add_stylesheet(shape_css(shape), StylesheetOrigin::Author);
    let root = doc.document_element().id();
    let mut groups = Vec::with_capacity(GROUPS);
    let mut last_leaf = None;
    for _ in 0..GROUPS {
        let group = doc.create_element("view", ());
        doc.add_class(group, "group");
        doc.append_child(root, group);
        for index in 0..LEAVES_PER_GROUP {
            let leaf = doc.create_element("view", ());
            doc.add_class(leaf, "leaf");
            if has_reader_leaf(shape) && index + 1 == LEAVES_PER_GROUP {
                doc.add_class(leaf, "reader");
            }
            doc.append_child(group, leaf);
            last_leaf = Some(leaf);
        }
        groups.push(group);
    }
    Page {
        doc,
        groups,
        leaf: last_leaf.expect("every page shape has leaves"),
    }
}

/// The same page with the first `layout()` — and, where the shape has one,
/// its container-unit loop — already paid for.
///
/// This is also where each shape is checked to resolve the width it was
/// written to resolve: input building is not timed, so a benchmark that
/// stopped measuring the case it names fails here rather than reporting a
/// cheaper number.
fn laid_out_page(shape: &str) -> Page {
    let mut page = page(shape);
    page.doc.layout();
    let width = page
        .doc
        .rounded_layout(page.leaf)
        .expect("the leaf is live")
        .size
        .width;
    assert!(
        (width - settled_leaf_width(shape)).abs() < f32::EPSILON,
        "{shape}: a leaf settled at {width}px, not {}px",
        settled_leaf_width(shape)
    );
    page
}

/// The first layout of a fresh document: the flush, the layout, and whatever
/// further passes the shape's containers owe.
#[divan::bench(args = FIRST_LAYOUT_SHAPES)]
fn first_layout(bencher: divan::Bencher, shape: &str) {
    // The measured body cannot check its own result without timing the check,
    // so one page is built and settled here instead.
    drop(laid_out_page(shape));
    bencher
        .with_inputs(|| page(shape))
        .counter(ItemsCount::new(LEAVES))
        .bench_local_values(|mut page| {
            page.doc.layout();
            black_box(page)
        });
}

/// Resizing every query container on a laid-out page: one inline style per
/// group, then one `layout()`.
#[divan::bench(args = RESIZE_SHAPES)]
fn container_resize(bencher: divan::Bencher, shape: &str) {
    bencher
        .with_inputs(|| laid_out_page(shape))
        .counter(ItemsCount::new(LEAVES))
        .bench_local_values(|mut page| {
            for &group in &page.groups {
                page.doc.set_inline_style(group, RESIZED_CONTAINER);
            }
            page.doc.layout();
            black_box(page)
        });
}

/// Resizing the viewport on a laid-out page. No container moves, so this is
/// the document-wide recascade with container units in it against the same
/// recascade without.
#[divan::bench(args = RESIZE_SHAPES)]
fn viewport_resize(bencher: divan::Bencher, shape: &str) {
    bencher
        .with_inputs(|| laid_out_page(shape))
        .counter(ItemsCount::new(LEAVES))
        .bench_local_values(|mut page| {
            page.doc
                .set_viewport(RESIZED_VIEWPORT_WIDTH, VIEWPORT_HEIGHT);
            page.doc.layout();
            black_box(page)
        });
}
