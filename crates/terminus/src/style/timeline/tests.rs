#![allow(clippy::float_cmp, reason = "the expectations are exact px")]

use euclid::default::Vector2D;
use stylo::servo::animation::AnimationProgress;
use stylo::values::computed::{AnimationDirection, AnimationFillMode};

use super::{Axis, Binding, ProgressTiming, iteration_progress};
use crate::test_common::Doc;
use crate::tree::document::NodeId;

fn timed(
    range: [f32; 2],
    duration: Option<f64>,
    delay: f64,
    iterations: f64,
    fill: AnimationFillMode,
) -> ProgressTiming {
    ProgressTiming::normalized(
        1000.0,
        range,
        duration,
        delay,
        iterations,
        AnimationDirection::Normal,
        fill,
    )
}

fn timing(range: [f32; 2], duration: Option<f64>, delay: f64, iterations: f64) -> ProgressTiming {
    timed(range, duration, delay, iterations, AnimationFillMode::None)
}

fn normalized(timing: ProgressTiming) -> (f64, f64, f64) {
    (
        timing.origin - f64::from(timing.start),
        timing.iteration,
        f64::from(timing.end) - timing.origin,
    )
}

#[test]
fn durations_and_delays_normalize_proportionally_into_the_range() {
    let whole = [0.0, 1000.0];
    for (case, timing, expected) in [
        ("auto", timing(whole, None, 0.0, 1.0), (0.0, 1000.0, 1000.0)),
        (
            "auto ignores the delay",
            timing(whole, None, 2.0, 1.0),
            (0.0, 1000.0, 1000.0),
        ),
        (
            "auto over 4",
            timing(whole, None, 0.0, 4.0),
            (0.0, 250.0, 1000.0),
        ),
        (
            "1s with a 1s delay",
            timing(whole, Some(1.0), 1.0, 1.0),
            (500.0, 500.0, 500.0),
        ),
        (
            "2s × 3 with 2s",
            timing(whole, Some(2.0), 2.0, 3.0),
            (250.0, 250.0, 750.0),
        ),
        (
            "a time duration, infinitely",
            timing(whole, Some(1.0), 1.0, f64::INFINITY),
            (0.0, 0.0, 0.0),
        ),
        (
            "auto, infinitely",
            timing(whole, None, 0.0, f64::INFINITY),
            (0.0, 0.0, 0.0),
        ),
        (
            "no iterations",
            timing(whole, None, 0.0, 0.0),
            (0.0, 0.0, 0.0),
        ),
        (
            "a total of nothing",
            timing(whole, Some(1.0), -1.0, 1.0),
            (0.0, 0.0, 0.0),
        ),
        (
            "no duration after a delay",
            timing(whole, Some(0.0), 1.0, 1.0),
            (1000.0, 0.0, 0.0),
        ),
        (
            "a negative delay",
            timing(whole, Some(2.0), -1.0, 1.0),
            (-1000.0, 2000.0, 2000.0),
        ),
        (
            "an empty range",
            timing([600.0, 400.0], Some(1.0), 1.0, 1.0),
            (0.0, 0.0, 0.0),
        ),
        (
            "a narrowed range",
            timing([200.0, 600.0], None, 0.0, 2.0),
            (0.0, 200.0, 400.0),
        ),
    ] {
        assert_eq!(normalized(timing), expected, "{case}");
    }
}

fn filled(
    fill: AnimationFillMode,
    direction: AnimationDirection,
    iterations: f64,
) -> ProgressTiming {
    ProgressTiming::normalized(
        1000.0,
        [200.0, 600.0],
        None,
        0.0,
        iterations,
        direction,
        fill,
    )
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "spelled as the `iteration_progress` it is compared with"
)]
fn at(progress: f64, reversed: bool) -> Option<AnimationProgress> {
    Some(AnimationProgress::new(progress, reversed))
}

#[test]
fn each_fill_mode_holds_only_its_own_phases() {
    let before = Some(AnimationProgress::before_phase(false));
    for (fill, expected) in [
        (AnimationFillMode::None, [None, at(0.5, false), None]),
        (
            AnimationFillMode::Forwards,
            [None, at(0.5, false), at(1.0, false)],
        ),
        (AnimationFillMode::Backwards, [before, at(0.5, false), None]),
        (
            AnimationFillMode::Both,
            [before, at(0.5, false), at(1.0, false)],
        ),
    ] {
        let timing = filled(fill, AnimationDirection::Normal, 1.0);
        let got = [100.0, 400.0, 800.0].map(|offset| iteration_progress(&timing, offset));
        assert_eq!(got, expected, "{fill:?}");
    }
}

#[test]
fn a_range_ending_at_the_timeline_end_stays_active_there() {
    let whole = timing([0.0, 1000.0], None, 0.0, 1.0);
    assert_eq!(iteration_progress(&whole, 1000.0), at(1.0, false));
    assert_eq!(iteration_progress(&whole, 0.0), at(0.0, false));
    let narrowed = filled(AnimationFillMode::None, AnimationDirection::Normal, 1.0);
    assert_eq!(
        iteration_progress(&narrowed, 600.0),
        None,
        "600 is inside the timeline"
    );
    assert_eq!(iteration_progress(&narrowed, 200.0), at(0.0, false));
}

#[test]
fn directions_report_the_iteration_they_reverse() {
    let quarter = 300.0;
    let reverse = filled(AnimationFillMode::None, AnimationDirection::Reverse, 1.0);
    assert_eq!(iteration_progress(&reverse, quarter), at(0.25, true));
    let alternate = filled(AnimationFillMode::Both, AnimationDirection::Alternate, 2.0);
    assert_eq!(iteration_progress(&alternate, 250.0), at(0.25, false));
    assert_eq!(iteration_progress(&alternate, 450.0), at(0.25, true));
    assert_eq!(
        iteration_progress(&alternate, 600.0),
        at(1.0, true),
        "the second ends reversed"
    );
    assert_eq!(iteration_progress(&alternate, 800.0), at(1.0, true));
    assert_eq!(
        iteration_progress(&alternate, 100.0),
        Some(AnimationProgress::before_phase(false))
    );
    let alternate_reverse = filled(
        AnimationFillMode::Both,
        AnimationDirection::AlternateReverse,
        2.0,
    );
    assert_eq!(
        iteration_progress(&alternate_reverse, 250.0),
        at(0.25, true)
    );
    assert_eq!(
        iteration_progress(&alternate_reverse, 450.0),
        at(0.25, false)
    );
    assert_eq!(
        iteration_progress(&alternate_reverse, 100.0),
        Some(AnimationProgress::before_phase(true))
    );
}

#[test]
fn an_infinite_count_sits_at_its_iteration_end() {
    let infinite = filled(
        AnimationFillMode::Both,
        AnimationDirection::Alternate,
        f64::INFINITY,
    );
    assert_eq!(iteration_progress(&infinite, 400.0), at(1.0, false));
    assert_eq!(iteration_progress(&infinite, 800.0), at(1.0, false));
    let none = filled(AnimationFillMode::Forwards, AnimationDirection::Normal, 0.0);
    assert_eq!(iteration_progress(&none, 400.0), at(0.0, false));
}

#[test]
fn a_delay_is_the_before_phase_in_proportion() {
    let before = Some(AnimationProgress::before_phase(false));
    for (fill, expected) in [
        (
            AnimationFillMode::Both,
            [before, at(0.0, false), at(0.5, false)],
        ),
        (
            AnimationFillMode::None,
            [None, at(0.0, false), at(0.5, false)],
        ),
    ] {
        let delayed = timed([0.0, 1000.0], Some(1.0), 1.0, 1.0, fill);
        let got = [250.0, 500.0, 750.0].map(|offset| iteration_progress(&delayed, offset));
        assert_eq!(got, expected, "{fill:?}");
    }
    let early = timed(
        [200.0, 600.0],
        Some(2.0),
        -1.0,
        1.0,
        AnimationFillMode::Both,
    );
    assert_eq!(iteration_progress(&early, 100.0), at(0.375, false));
    assert_eq!(iteration_progress(&early, 400.0), at(0.75, false));
    let early = timed(
        [200.0, 600.0],
        Some(2.0),
        -1.0,
        1.0,
        AnimationFillMode::None,
    );
    assert_eq!(iteration_progress(&early, 100.0), None);
}

#[test]
fn a_duration_and_delay_totalling_nothing_ends_at_the_range_start() {
    let both = timed([0.0, 1000.0], Some(1.0), -1.0, 1.0, AnimationFillMode::Both);
    assert_eq!(iteration_progress(&both, 500.0), at(1.0, false));
    let none = timed([0.0, 1000.0], Some(1.0), -1.0, 1.0, AnimationFillMode::None);
    assert_eq!(iteration_progress(&none, 500.0), None);
}

#[test]
fn the_timeline_boundary_is_the_scroll_limit() {
    let fill = AnimationFillMode::None;
    let direction = AnimationDirection::Normal;
    let entry = ProgressTiming::normalized(150.0, [100.0, 150.0], None, 0.0, 1.0, direction, fill);
    assert_eq!(iteration_progress(&entry, 150.0), at(1.0, false));
    let cover = ProgressTiming::normalized(650.0, [100.0, 350.0], None, 0.0, 1.0, direction, fill);
    assert_eq!(iteration_progress(&cover, 350.0), None);
}

#[test]
fn the_range_end_is_exact_under_a_rounded_delay() {
    let start = 0.2_f32 * 314.0;
    let timing = ProgressTiming::normalized(
        314.0,
        [start, 314.0],
        Some(2.0),
        1.0,
        1.0,
        AnimationDirection::Normal,
        AnimationFillMode::None,
    );
    assert_eq!(iteration_progress(&timing, 314.0), at(1.0, false));
}

#[test]
fn the_last_iteration_ends_at_one() {
    let thirds = timing([0.0, 1000.0], None, 0.0, 3.0);
    assert_eq!(iteration_progress(&thirds, 1000.0), at(1.0, false));
    let filled = timed([0.0, 700.0], None, 0.0, 3.0, AnimationFillMode::Forwards);
    assert_eq!(iteration_progress(&filled, 700.0), at(1.0, false));
    assert_eq!(iteration_progress(&filled, 900.0), at(1.0, false));
    let half = timing([0.0, 1000.0], None, 0.0, 1.5);
    assert_eq!(iteration_progress(&half, 1000.0), at(0.5, false));
}

const PAGE: &str = "
    page { display: flex; width: 800px; height: 600px; align-items: flex-start; }
    .scroller { display: flex; flex-direction: column; overflow: scroll; width: 200px;
                height: 200px; flex-shrink: 0; }
    .spacer { flex-shrink: 0; width: 200px; height: 300px; }
    .subject { flex-shrink: 0; width: 200px; height: 50px; }
    .tall { height: 300px; }
    .tail { flex-shrink: 0; width: 200px; height: 500px; }
    .box { display: flex; flex-direction: column; width: 200px; }
    .mover { flex-shrink: 0; width: 20px; height: 20px; }
    .wide { flex-shrink: 0; width: 900px; height: 20px; }
    .animated { animation: fade linear both; }
    @keyframes fade { from { opacity: 0; } to { opacity: 1; } }
    @keyframes dim { from { opacity: 0.25; } to { opacity: 0.75; } }";

fn binding(doc: &Doc, id: NodeId) -> Binding {
    doc.dom
        .animations()
        .timelines
        .binding(id, "fade")
        .cloned()
        .expect("the animation is bound")
}

fn attached(doc: &Doc, id: NodeId) -> Option<(NodeId, Axis, [f32; 2])> {
    match binding(doc, id) {
        Binding::Active {
            source,
            axis,
            timing,
            ..
        } => Some((source, axis, [timing.start, timing.end])),
        Binding::Inactive => None,
    }
}

fn view_page(scroller: &str, subject: &str) -> (Doc, NodeId, NodeId) {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let container = doc.el(root, "view.scroller");
    doc.set_inline(container, scroller);
    doc.el(container, "view.spacer");
    let element = doc.el(container, "view.subject.animated");
    doc.set_inline(element, subject);
    doc.el(container, "view.tail");
    doc.flush();
    (doc, container, element)
}

fn view_range(scroller: &str, subject: &str) -> [f32; 2] {
    let (doc, container, element) = view_page(scroller, subject);
    let (source, axis, range) = attached(&doc, element).expect("the view timeline is active");
    assert_eq!((source, axis), (container, Axis::Y));
    range
}

#[test]
fn view_ranges_follow_the_subject_through_the_scrollport() {
    for (subject, ranges) in [
        (
            "",
            [
                ("cover", [100.0, 350.0]),
                ("contain", [150.0, 300.0]),
                ("entry", [100.0, 150.0]),
                ("exit", [300.0, 350.0]),
                ("entry-crossing", [100.0, 150.0]),
                ("exit-crossing", [300.0, 350.0]),
                ("scroll", [0.0, 650.0]),
                ("normal", [100.0, 350.0]),
                ("contain 50% exit 50%", [225.0, 325.0]),
                ("25% 100px", [162.5, 200.0]),
            ],
        ),
        (
            "height: 300px",
            [
                ("cover", [100.0, 600.0]),
                ("contain", [300.0, 400.0]),
                ("entry", [100.0, 300.0]),
                ("exit", [400.0, 600.0]),
                ("entry-crossing", [100.0, 400.0]),
                ("exit-crossing", [300.0, 600.0]),
                ("scroll", [0.0, 900.0]),
                ("normal", [100.0, 600.0]),
                ("contain 50% exit 50%", [350.0, 500.0]),
                ("25% 300px", [225.0, 400.0]),
            ],
        ),
    ] {
        for (range, expected) in ranges {
            let got = view_range(
                "",
                &format!("{subject}; animation-timeline: view(); animation-range: {range}"),
            );
            assert_eq!(got, expected, "{subject:?}: {range}");
        }
    }
}

#[test]
fn view_insets_narrow_the_visibility_range() {
    for (scroller, timeline, expected) in [
        ("", "view(20px 40px)", [140.0, 330.0]),
        ("", "view(10%)", [120.0, 330.0]),
        (
            "scroll-padding: 30px 0px 50px 0px",
            "view()",
            [150.0, 320.0],
        ),
        (
            "scroll-padding: 30px 0px 50px 0px",
            "view(auto 0px)",
            [100.0, 320.0],
        ),
    ] {
        assert_eq!(
            view_range(scroller, &format!("animation-timeline: {timeline}")),
            expected,
            "{scroller}: {timeline}"
        );
    }
}

#[test]
fn a_sticky_subject_takes_the_earliest_or_latest_alignment() {
    let sticky = "position: sticky; top: 0px; animation-timeline: view()";
    for (range, expected) in [
        ("cover", [100.0, 850.0]),
        ("contain", [150.0, 800.0]),
        ("entry-crossing", [100.0, 150.0]),
        ("exit-crossing", [800.0, 850.0]),
        ("exit", [800.0, 850.0]),
    ] {
        assert_eq!(
            view_range("", &format!("{sticky}; animation-range: {range}")),
            expected,
            "{range}"
        );
    }
}

#[test]
fn a_bottom_sticky_subject_takes_the_earliest_or_latest_alignment() {
    let sticky = "position: sticky; bottom: 0px";
    for (timeline, range, expected) in [
        ("view()", "cover", [-200.0, 350.0]),
        ("view()", "contain", [-150.0, 300.0]),
        ("view()", "entry", [-200.0, -150.0]),
        ("view()", "entry-crossing", [-200.0, -150.0]),
        ("view()", "exit", [300.0, 350.0]),
        ("view(0px 50px)", "cover", [150.0, 350.0]),
        ("view(0px 50px)", "entry", [150.0, 200.0]),
        ("view(0px 50px)", "entry-crossing", [150.0, 200.0]),
        ("view(0px 50px)", "contain", [200.0, 300.0]),
    ] {
        assert_eq!(
            view_range(
                "",
                &format!("{sticky}; animation-timeline: {timeline}; animation-range: {range}")
            ),
            expected,
            "{timeline} {range}"
        );
    }
}

#[test]
fn a_top_sticky_subject_ends_cover_at_the_earliest_alignment() {
    let sticky = "position: sticky; top: 0px; animation-timeline: view(50px 0px)";
    for (range, expected) in [
        ("cover", [100.0, 300.0]),
        ("exit", [250.0, 300.0]),
        ("exit-crossing", [250.0, 300.0]),
        ("contain", [150.0, 250.0]),
    ] {
        assert_eq!(
            view_range("", &format!("{sticky}; animation-range: {range}")),
            expected,
            "{range}"
        );
    }
}

#[test]
fn a_sticky_chain_moves_the_subject_by_every_box() {
    for (range, expected) in [
        ("cover", [150.0, 890.0]),
        ("contain", [200.0, 840.0]),
        ("entry", [150.0, 200.0]),
        ("exit", [840.0, 890.0]),
        ("entry-crossing", [150.0, 200.0]),
        ("exit-crossing", [840.0, 890.0]),
    ] {
        let mut doc = Doc::with_css(PAGE);
        let root = doc.root;
        let container = doc.el(root, "view.scroller");
        doc.el(container, "view.spacer");
        let outer = doc.el(container, "view.box");
        doc.set_inline(
            outer,
            "position: sticky; top: 0px; height: 150px; flex-shrink: 0",
        );
        doc.el(outer, "view.subject");
        let element = doc.el(outer, "view.subject.animated");
        doc.set_inline(
            element,
            &format!(
                "position: sticky; top: 60px; animation-timeline: view(60px 0px);
                 animation-range: {range}"
            ),
        );
        doc.el(outer, "view.subject");
        doc.el(container, "view.tail");
        doc.flush();
        let (source, _, got) = attached(&doc, element).expect("active");
        assert_eq!((source, got), (container, expected), "{range}");
    }
}

#[test]
fn a_view_subject_in_skipped_contents_is_inactive() {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let container = doc.el(root, "view.scroller");
    doc.el(container, "view.spacer");
    let hidden = doc.el(container, "view.subject");
    doc.set_inline(hidden, "content-visibility: hidden");
    let card = doc.el(hidden, "view.mover");
    doc.set_inline(card, "view-timeline-name: --t");
    doc.el(container, "view.tail");
    let mover = doc.el(root, "view.mover.animated");
    doc.set_inline(mover, "animation-timeline: --t");
    doc.flush();
    assert_eq!(attached(&doc, mover), None, "skipped");
    doc.set_inline(hidden, "");
    doc.flush();
    assert_eq!(source(&doc, mover), Some(container), "rendered");
}

#[test]
fn scroll_functions_find_their_scroller() {
    let mut doc = Doc::with_css(&format!(
        "{PAGE}
         .across {{ overflow-x: scroll; overflow-y: hidden; }}
         .row {{ display: flex; flex-direction: row; }}"
    ));
    let root = doc.root;
    let outer = doc.el(root, "view.scroller");
    doc.el(outer, "view.tail");
    let across = doc.el(outer, "view.scroller.across.row");
    doc.el(across, "view.wide");
    let block = doc.el(across, "view.mover.animated");
    doc.set_inline(block, "animation-timeline: scroll()");
    let inline = doc.el(across, "view.mover.animated");
    doc.set_inline(inline, "animation-timeline: scroll(inline)");
    let root_axis = doc.el(across, "view.mover.animated");
    doc.set_inline(root_axis, "animation-timeline: scroll(root)");
    let own = doc.el(outer, "view.scroller.animated");
    doc.set_inline(own, "animation-timeline: scroll(self); flex-shrink: 0");
    doc.el(own, "view.tail");
    let unscrolled = doc.el(outer, "view.scroller.animated");
    doc.set_inline(
        unscrolled,
        "animation-timeline: scroll(self); flex-shrink: 0",
    );
    let fixed = doc.el(outer, "view.mover.animated");
    doc.set_inline(fixed, "position: fixed; animation-timeline: scroll()");
    let fixed_view = doc.el(outer, "view.mover.animated");
    doc.set_inline(fixed_view, "position: fixed; animation-timeline: view()");
    doc.flush();

    assert_eq!(
        attached(&doc, block),
        None,
        "the horizontal scroller's hidden y axis captures scroll(block), and it has no range"
    );
    let (source, axis, _) = attached(&doc, inline).expect("scroll(inline) is active");
    assert_eq!((source, axis), (across, Axis::X));
    assert_eq!(attached(&doc, root_axis), None, "the page does not scroll");
    let (source, _, range) = attached(&doc, own).expect("scroll(self) is active");
    assert_eq!((source, range), (own, [0.0, 300.0]));
    assert_eq!(
        attached(&doc, unscrolled),
        None,
        "nothing to scroll: inactive"
    );
    assert_eq!(attached(&doc, fixed), None, "no scroller above a fixed box");
    assert_eq!(
        attached(&doc, fixed_view),
        None,
        "view() has no root fallback"
    );

    doc.set_inline(root, "overflow: scroll; flex-direction: column");
    doc.el(root, "view.tail.tall");
    let direct = doc.el(root, "view.mover.animated");
    doc.set_inline(direct, "animation-timeline: scroll()");
    doc.flush();
    let (source, _, _) = attached(&doc, root_axis).expect("the scrolling page is the root");
    assert_eq!(source, root);
    let (source, _, _) = attached(&doc, direct).expect("nearest falls back to the root");
    assert_eq!(source, root);
}

fn named(css: &str, build: impl FnOnce(&mut Doc) -> NodeId) -> (Doc, NodeId) {
    let mut doc = Doc::with_css(&format!(
        "{PAGE}
         .t {{ scroll-timeline: --t; }}
         .scoped {{ timeline-scope: --t; }}
         .named {{ animation-timeline: --t; }}
         {css}"
    ));
    let mover = build(&mut doc);
    doc.flush();
    (doc, mover)
}

fn scroller(doc: &mut Doc, parent: NodeId, classes: &str) -> NodeId {
    let id = doc.el(parent, &format!("view.scroller{classes}"));
    doc.el(id, "view.tail");
    id
}

fn source(doc: &Doc, mover: NodeId) -> Option<NodeId> {
    attached(doc, mover).map(|(source, _, _)| source)
}

#[test]
fn named_timelines_resolve_by_ancestry_then_scope() {
    let (doc, mover) = named("", |doc| {
        let root = doc.root;
        scroller(doc, root, ".t.named.animated")
    });
    assert_eq!(source(&doc, mover), Some(mover), "its own timeline");

    let mut expected = None;
    let (doc, mover) = named("", |doc| {
        let root = doc.root;
        let outer = scroller(doc, root, ".t");
        let inner = scroller(doc, outer, "");
        scroller(doc, root, ".t");
        expected = Some(outer);
        doc.el(inner, "view.mover.named.animated")
    });
    assert_eq!(
        source(&doc, mover),
        expected,
        "an ancestor beats a later sibling"
    );

    for scoped in [true, false] {
        let mut expected = None;
        let (doc, mover) = named("", |doc| {
            let root = doc.root;
            let boundary = doc.el(
                root,
                if scoped {
                    "view.box.scoped"
                } else {
                    "view.box"
                },
            );
            scroller(doc, boundary, ".t");
            let second = scroller(doc, boundary, ".t");
            let mover = doc.el(boundary, "view.mover.named.animated");
            let outside = scroller(doc, root, ".t");
            expected = Some(if scoped { second } else { outside });
            mover
        });
        assert_eq!(
            source(&doc, mover),
            expected,
            "the last in tree order under the scope (scoped: {scoped})"
        );
    }

    let mut expected = None;
    let (doc, mover) = named("", |doc| {
        let root = doc.root;
        let boundary = doc.el(root, "view.box.scoped");
        let first = scroller(doc, boundary, ".t");
        let nested = doc.el(boundary, "view.box.scoped");
        scroller(doc, nested, ".t");
        expected = Some(first);
        doc.el(boundary, "view.mover.named.animated")
    });
    assert_eq!(
        source(&doc, mover),
        expected,
        "a nested scope keeps its own"
    );

    let mut expected = None;
    let (doc, mover) = named("", |doc| {
        let root = doc.root;
        let boundary = doc.el(root, "view.box.scoped");
        let outer = scroller(doc, boundary, ".t");
        expected = Some(scroller(doc, outer, ".t"));
        doc.el(boundary, "view.mover.named.animated")
    });
    assert_eq!(
        source(&doc, mover),
        expected,
        "a descendant follows its ancestor"
    );

    let (doc, mover) = named("", |doc| {
        let root = doc.root;
        doc.el(root, "view.mover.named.animated")
    });
    assert_eq!(attached(&doc, mover), None, "no definer: inactive");
}

#[test]
fn one_definer_resolves_one_timeline() {
    for (declaration, axis) in [
        ("scroll-timeline: --t inline, --t block", Axis::Y),
        ("scroll-timeline: --t block, --t inline", Axis::X),
    ] {
        let mut definer = None;
        let (doc, mover) = named(&format!(".later {{ {declaration}; }}"), |doc| {
            let root = doc.root;
            let id = scroller(doc, root, ".later");
            doc.el(id, "view.wide");
            definer = Some(id);
            doc.el(id, "view.mover.named.animated")
        });
        assert_eq!(
            attached(&doc, mover).map(|(source, axis, _)| (source, axis)),
            definer.map(|definer| (definer, axis)),
            "{declaration}"
        );
    }

    let mut both = None;
    let (doc, mover) = named(".both { view-timeline-name: --t; }", |doc| {
        let root = doc.root;
        let outer = scroller(doc, root, "");
        let id = scroller(doc, outer, ".t.both");
        both = Some(id);
        doc.el(id, "view.mover.named.animated")
    });
    assert_eq!(source(&doc, mover), both, "the scroll timeline wins");

    let (doc, mover) = named(".flat { scroll-timeline: --t; }", |doc| {
        let root = doc.root;
        let flat = doc.el(root, "view.box.flat");
        let inner = scroller(doc, flat, ".t");
        doc.el(inner, "view.mover.named.animated");
        doc.el(flat, "view.mover.named.animated")
    });
    assert_eq!(
        attached(&doc, mover),
        None,
        "a non-scrolling definer is inactive"
    );
}

#[test]
fn a_shadow_tree_matches_only_its_own_names() {
    for (document_css, shadow_css, bound) in [
        (".host { scroll-timeline: --t; }", "", false),
        ("", ":host { scroll-timeline: --t; }", true),
    ] {
        let mut doc = Doc::with_css(&format!("{PAGE} {document_css}"));
        let root = doc.root;
        let host = doc.el(root, "view.scroller.host");
        let shadow = doc
            .dom
            .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        doc.dom.add_shadow_stylesheet(
            shadow,
            &format!(
                "{shadow_css}
                 @keyframes fade {{ from {{ opacity: 0; }} to {{ opacity: 1; }} }}
                 .inside {{ flex-shrink: 0; width: 20px; height: 600px;
                            animation: fade linear both; animation-timeline: --t; }}"
            ),
        );
        let inside = doc.el(shadow, "view.inside");
        doc.flush();
        assert_eq!(
            source(&doc, inside),
            bound.then_some(host),
            "{document_css}{shadow_css}"
        );
    }
}

#[test]
fn a_slotted_name_belongs_to_the_slot_tree() {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let outer = doc.el(root, "view.box");
    let tree = doc
        .dom
        .attach_shadow(outer, crate::tree::shadow::ShadowRootMode::Open);
    let keyframes = "@keyframes fade { from { opacity: 0; } to { opacity: 1; } }
         .inside { flex-shrink: 0; width: 20px; height: 20px;
                   animation: fade linear both; animation-timeline: --t; }";
    doc.dom.add_shadow_stylesheet(
        tree,
        &format!("::slotted(*) {{ scroll-timeline: --t; }} {keyframes}"),
    );
    doc.el(tree, "slot");
    let from_slot_tree = doc.el(tree, "view.inside");
    let slotted = doc.el(outer, "view.scroller");
    let own = doc
        .dom
        .attach_shadow(slotted, crate::tree::shadow::ShadowRootMode::Open);
    doc.dom.add_shadow_stylesheet(
        own,
        &format!(".tall {{ flex-shrink: 0; width: 200px; height: 500px; }} {keyframes}"),
    );
    doc.el(own, "view.tall");
    let from_own_tree = doc.el(own, "view.inside");
    doc.flush();
    assert_eq!(
        source(&doc, from_slot_tree),
        Some(slotted),
        "the slot's tree"
    );
    assert_eq!(attached(&doc, from_own_tree), None, "its own shadow tree");
}

#[test]
fn a_binding_samples_the_live_offset() {
    let (mut doc, container, element) = view_page("", "animation-timeline: view()");
    doc.dom.scroll_to(container, Vector2D::new(0.0, 225.0));
    doc.dom.advance_scroll_timelines(&[container]);
    assert_eq!(doc.value(element, "opacity"), "0.5");
}

#[test]
fn a_scroll_timeline_through_the_seam_reads_the_nearer_end_of_the_range() {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let container = doc.el(root, "view.scroller");
    doc.set_inline(container, "overscroll-behavior-y: circular");
    let subject = doc.el(container, "view.subject.animated");
    doc.set_inline(subject, "animation-timeline: view()");
    let mover = doc.el(container, "view.mover.animated");
    doc.set_inline(mover, "animation-timeline: scroll()");
    doc.el(container, "view.tail");
    doc.flush();
    let scroll_box = doc.dom.scroll_box(container).expect("a scroll box");
    assert_eq!(scroll_box.wrap_period(), (None, Some(570.0)));
    for (offset, scroll, view) in [
        (550.0, "0", "1"),
        (390.0, "1", "1"),
        (470.0, "1", "1"),
        (185.0, "0.5", "1"),
        (0.0, "0", "0.8"),
    ] {
        assert_eq!(
            doc.dom.scroll_to(container, Vector2D::new(0.0, offset)),
            Vector2D::new(0.0, offset)
        );
        doc.dom.advance_scroll_timelines(&[container]);
        assert_eq!(doc.value(mover, "opacity"), scroll, "scroll() at {offset}");
        assert_eq!(doc.value(subject, "opacity"), view, "view() at {offset}");
    }
}

#[test]
fn a_view_range_holds_its_end_only_at_the_scroll_limit() {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let container = doc.el(root, "view.scroller");
    doc.el(container, "view.spacer");
    let last = doc.el(container, "view.subject");
    doc.set_inline(
        last,
        "animation: dim linear; animation-timeline: view(); animation-range: entry",
    );
    doc.flush();
    doc.dom.scroll_to(container, Vector2D::new(0.0, 150.0));
    doc.dom.advance_scroll_timelines(&[container]);
    assert_eq!(
        doc.value(last, "opacity"),
        "0.75",
        "entry ends at the limit"
    );

    let (mut doc, container, element) =
        view_page("", "animation: dim linear; animation-timeline: view()");
    doc.dom.scroll_to(container, Vector2D::new(0.0, 350.0));
    doc.dom.advance_scroll_timelines(&[container]);
    assert_eq!(doc.value(element, "opacity"), "1", "cover ends short of it");
}

#[test]
fn only_an_active_timeline_makes_its_animation_current() {
    let mut doc = Doc::with_css(PAGE);
    let root = doc.root;
    let idle = doc.el(root, "view.mover.animated");
    doc.set_inline(idle, "animation-timeline: scroll()");
    let container = doc.el(root, "view.scroller");
    let current = doc.el(container, "view.mover.animated");
    doc.set_inline(current, "animation-timeline: scroll()");
    doc.el(container, "view.tail");
    doc.flush();
    let animates = |id| doc.dom.get(id).expect("live").animates_opacity();
    assert!(!animates(idle), "inactive: idle");
    assert!(animates(current), "active: current");
}

#[test]
fn a_definer_leaves_with_its_element_and_returns_with_it() {
    let mut definer = None;
    let (mut doc, mover) = named("", |doc| {
        let root = doc.root;
        let id = scroller(doc, root, ".t");
        definer = Some(id);
        doc.el(root, "view.mover.named.animated")
    });
    let definer = definer.expect("built");
    assert_eq!(source(&doc, mover), Some(definer));
    let listed = |doc: &Doc| doc.dom.animations().timelines.defines(definer);
    assert!(listed(&doc));
    doc.dom.remove_element(definer);
    doc.flush();
    assert_eq!(attached(&doc, mover), None, "unlinked");
    assert!(!listed(&doc), "pruned on unlink");
    let root = doc.root;
    doc.dom.append_child(root, definer);
    doc.flush();
    assert_eq!(source(&doc, mover), Some(definer), "linked back");
    assert!(listed(&doc));
}

fn range_page(keyframes: &str, animation: &str) -> (Doc, NodeId, NodeId) {
    let mut doc = Doc::with_css(&format!(
        "page {{ display: flex; width: 800px; height: 600px; align-items: flex-start; }}
         .container {{ display: flex; flex-direction: column; overflow: scroll; width: 400px;
                       height: 400px; flex-shrink: 0; }}
         .spacer {{ flex-shrink: 0; width: 400px; height: 600px; }}
         .subject {{ flex-shrink: 0; width: 200px; height: 200px; view-timeline-name: --foo; }}
         @keyframes fade-in-out {{
             entry 0%, exit 100% {{ opacity: 0 }}
             entry 100%, exit 0% {{ opacity: 1 }} }}
         {keyframes}"
    ));
    let root = doc.root;
    let container = doc.el(root, "view.container");
    doc.el(container, "view.spacer");
    let subject = doc.el(container, "view.subject");
    doc.set_inline(
        subject,
        &format!("animation: {animation}; animation-timeline: --foo"),
    );
    doc.el(container, "view.spacer");
    doc.flush();
    (doc, container, subject)
}

fn value_at(doc: &mut Doc, container: NodeId, id: NodeId, offset: f32, longhand: &str) -> String {
    doc.dom.scroll_to(container, Vector2D::new(0.0, offset));
    doc.dom.advance_scroll_timelines(&[container]);
    doc.value(id, longhand)
}

fn opacity_at(doc: &mut Doc, container: NodeId, id: NodeId, offset: f32) -> String {
    value_at(doc, container, id, offset, "opacity")
}

#[test]
fn range_keyframes_sit_on_their_named_ranges() {
    let (mut doc, container, subject) = range_page("", "fade-in-out linear both");
    for (offset, expected) in [
        (200.0, "0"),
        (300.0, "0.5"),
        (400.0, "1"),
        (500.0, "1"),
        (600.0, "1"),
        (700.0, "0.5"),
        (800.0, "0"),
    ] {
        assert_eq!(
            opacity_at(&mut doc, container, subject, offset),
            expected,
            "at {offset}"
        );
    }
    assert_eq!(opacity_at(&mut doc, container, subject, 400.0), "1");
    doc.set_inline(container, "height: 300px");
    doc.flush();
    assert_eq!(
        doc.value(subject, "opacity"),
        "0.5",
        "entry 50% after the resize"
    );
    assert_eq!(
        opacity_at(&mut doc, container, subject, 700.0),
        "0.5",
        "exit 50% after the resize"
    );
}

#[test]
fn range_keyframes_equal_the_two_animation_formulation() {
    let keyframes = "
        @keyframes animate-in-and-out {
            entry 0% { opacity: 0; transform: translateY(100%); }
            entry 100% { opacity: 1; transform: translateY(0); }
            exit 0% { opacity: 1; transform: translateY(0); }
            exit 100% { opacity: 0; transform: translateY(-100%); } }
        @keyframes animate-in {
            0% { opacity: 0; transform: translateY(100%); }
            100% { opacity: 1; transform: translateY(0); } }
        @keyframes animate-out {
            0% { opacity: 1; transform: translateY(0); }
            100% { opacity: 0; transform: translateY(-100%); } }";
    let (mut ranged, ranged_container, ranged_subject) =
        range_page(keyframes, "linear animate-in-and-out");
    let (mut split, split_container, split_subject) = range_page(
        keyframes,
        "animate-in linear forwards, animate-out linear forwards; animation-range: entry, exit",
    );
    for offset in [
        200.0, 250.0, 300.0, 399.0, 400.0, 500.0, 600.0, 650.0, 700.0, 799.0,
    ] {
        for longhand in ["opacity", "transform"] {
            assert_eq!(
                value_at(
                    &mut ranged,
                    ranged_container,
                    ranged_subject,
                    offset,
                    longhand
                ),
                value_at(&mut split, split_container, split_subject, offset, longhand),
                "{longhand} at {offset}"
            );
        }
    }
    assert_eq!(
        value_at(
            &mut ranged,
            ranged_container,
            ranged_subject,
            250.0,
            "opacity"
        ),
        "0.25"
    );
}

#[test]
fn range_keyframes_outside_the_attachment_range_interpolate_across_it() {
    let mut doc = Doc::with_css(
        "page { display: flex; width: 800px; height: 600px; align-items: flex-start; }
         .scroller { display: flex; flex-direction: column; overflow: scroll; width: 300px;
                     height: 200px; flex-shrink: 0; border: 10px solid black; }
         .target { flex-shrink: 0; margin: 800px 10px 0px; width: 100px; height: 100px;
                   animation: anim auto both linear; animation-timeline: --t1;
                   animation-range-start: contain 0%; animation-range-end: contain 100%;
                   view-timeline: --t1 block; }
         @keyframes anim {
             cover 0% { opacity: 0; transform: none; margin-left: 0px; }
             cover 100% { opacity: 1; transform: translateX(300px); margin-right: 0px; } }
         .tail { flex-shrink: 0; width: 100px; height: 800px; }",
    );
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let target = doc.el(scroller, "view.target");
    doc.el(scroller, "view.tail");
    doc.flush();
    for (offset, expected) in [
        (650.0, ["translateX(100px)", "0.333333", "5px", "10px"]),
        (700.0, ["translateX(100px)", "0.333333", "5px", "10px"]),
        (750.0, ["translateX(150px)", "0.5", "7.5px", "7.5px"]),
        (800.0, ["translateX(200px)", "0.666667", "10px", "5px"]),
    ] {
        let got = ["transform", "opacity", "margin-left", "margin-right"]
            .map(|longhand| value_at(&mut doc, scroller, target, offset, longhand));
        assert_eq!(got, expected, "at {offset}");
    }
}

#[test]
fn range_keyframes_order_by_offset_then_computed_order() {
    let keyframes = "
        @keyframes tie {
            from { opacity: 0; }
            entry 100% { opacity: 0.2; }
            contain 0% { opacity: 0.8; }
            to { opacity: 1; } }
        @keyframes order { cover 50% { opacity: 0.8; } 50% { opacity: 0.2; } }
        @keyframes merged {
            entry 100% { opacity: 0.4; }
            contain 0% { opacity: 0.9; }
            entry 100% { opacity: 0.5; } }";
    let (mut doc, container, subject) = range_page(keyframes, "tie linear both");
    for (offset, expected) in [
        (300.0, "0.1"),
        (399.0, "0.199"),
        (400.0, "0.8"),
        (500.0, "0.85"),
    ] {
        assert_eq!(
            opacity_at(&mut doc, container, subject, offset),
            expected,
            "tie at {offset}"
        );
    }
    let (mut doc, container, subject) = range_page(keyframes, "order linear both");
    assert_eq!(
        opacity_at(&mut doc, container, subject, 500.0),
        "0.8",
        "the range keyframe follows the percentage keyframe at 50%"
    );
    assert_eq!(opacity_at(&mut doc, container, subject, 350.0), "0.6");
    let (mut doc, container, subject) = range_page(keyframes, "merged linear both");
    for (offset, expected) in [(300.0, "0.75"), (400.0, "0.9"), (500.0, "0.925")] {
        assert_eq!(
            opacity_at(&mut doc, container, subject, offset),
            expected,
            "merged at {offset}"
        );
    }
}

fn sampled_opacity(doc: &Doc, id: NodeId, name: &str, progress: f64) -> Option<f32> {
    use stylo::properties::animated_properties::{AnimationValue, AnimationValueMap};
    use stylo::properties::{LonghandId, OwnedPropertyDeclarationId};
    let handle = doc.dom.animations().context_handle();
    let sets = handle.sets.read();
    let key = stylo::servo::animation::AnimationSetKey::new_for_non_pseudo(stylo::dom::OpaqueNode(
        id.arena_key(),
    ));
    let animation = sets
        .get(&key)?
        .animations
        .iter()
        .find(|animation| &*animation.name == name)?;
    let mut values = AnimationValueMap::default();
    animation.sample_at(AnimationProgress::new(progress, false), &mut values);
    match values.get(&OwnedPropertyDeclarationId::Longhand(LonghandId::Opacity))? {
        AnimationValue::Opacity(opacity) => Some(*opacity),
        _ => None,
    }
}

#[test]
fn range_keyframes_are_ignored_on_the_document_timeline() {
    let mut doc = Doc::with_css(
        "page { display: flex; width: 800px; height: 600px; }
         @keyframes mixed { entry 0% { opacity: 0.2; } 50% { width: 20px; } }",
    );
    let root = doc.root;
    let element = doc.el(root, "view");
    doc.set_inline(
        element,
        "opacity: 0.7; width: 10px; height: 10px; flex-shrink: 0;
         animation: mixed 2s linear -1s both paused",
    );
    doc.flush();
    assert_eq!(doc.value(element, "width"), "20px");
    assert_eq!(doc.value(element, "opacity"), "0.7");
    assert_eq!(sampled_opacity(&doc, element, "mixed", 0.25), None);
}

#[test]
fn range_keyframes_on_a_scroll_timeline_name_the_whole_range() {
    let mut doc = Doc::with_css(&format!(
        "{PAGE}
         @keyframes anim {{ cover 100% {{ opacity: 1; }} cover 0% {{ opacity: 0; }}
                            50% {{ opacity: 0.5; }} }}"
    ));
    let root = doc.root;
    let container = doc.el(root, "view.scroller");
    let mover = doc.el(container, "view.mover");
    doc.set_inline(
        mover,
        "animation: anim linear both; animation-timeline: scroll()",
    );
    doc.el(container, "view.tail");
    doc.el(container, "view.tail");
    doc.flush();
    for (offset, expected) in [(0.0, "0"), (205.0, "0.25"), (615.0, "0.75"), (820.0, "1")] {
        assert_eq!(
            opacity_at(&mut doc, container, mover, offset),
            expected,
            "at {offset}"
        );
    }
}

#[test]
fn range_keyframes_follow_a_resized_subject() {
    let (mut doc, container, subject) = range_page("", "fade-in-out linear both");
    assert_eq!(opacity_at(&mut doc, container, subject, 300.0), "0.5");
    doc.set_inline(
        subject,
        "height: 100px; animation: fade-in-out linear both; animation-timeline: --foo",
    );
    doc.flush();
    assert_eq!(doc.value(subject, "opacity"), "1");
}

#[test]
fn a_restyle_keeps_the_placed_range_keyframes() {
    let (mut doc, container, subject) = range_page("", "fade-in-out linear both");
    assert_eq!(opacity_at(&mut doc, container, subject, 300.0), "0.5");
    doc.set_inline(
        subject,
        "background-color: red; animation: fade-in-out linear both; animation-timeline: --foo",
    );
    doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
    assert_eq!(doc.value(subject, "background-color"), "rgb(255, 0, 0)");
    assert_eq!(doc.value(subject, "opacity"), "0.5");
}

#[test]
fn a_paused_animation_places_its_range_keyframes_again() {
    let (mut doc, container, subject) = range_page("", "fade-in-out linear both");
    assert_eq!(opacity_at(&mut doc, container, subject, 300.0), "0.5");
    let paused = "animation: fade-in-out linear both paused; animation-timeline: --foo";
    doc.set_inline(subject, paused);
    doc.flush();
    assert_eq!(doc.value(subject, "opacity"), "0.5");
    doc.set_inline(subject, &format!("height: 100px; {paused}"));
    doc.flush();
    assert_eq!(doc.value(subject, "opacity"), "0.833333");
    assert_eq!(
        opacity_at(&mut doc, container, subject, 700.0),
        "0.833333",
        "paused: the scroll does not move it"
    );
}

#[test]
fn range_keyframes_in_skipped_contents_hold() {
    let (mut doc, container, subject) = range_page("", "none");
    let root = doc.root;
    let hidden = doc.el(root, "view");
    let element = doc.el(hidden, "view");
    doc.set_inline(
        element,
        "width: 10px; height: 10px; animation: fade-in-out linear both;
         animation-timeline: --foo",
    );
    doc.flush();
    assert_eq!(opacity_at(&mut doc, container, element, 300.0), "0.5");
    assert_eq!(
        sampled_opacity(&doc, element, "fade-in-out", 1.0 / 6.0),
        Some(0.5)
    );
    doc.set_inline(hidden, "content-visibility: hidden");
    doc.set_inline(subject, "height: 100px");
    doc.flush();
    assert_eq!(
        sampled_opacity(&doc, element, "fade-in-out", 1.0 / 6.0),
        Some(0.5),
        "skipped: placed for the 200px subject"
    );
    doc.set_inline(hidden, "");
    doc.flush();
    assert_eq!(
        sampled_opacity(&doc, element, "fade-in-out", 1.0 / 6.0),
        Some(0.833_333_3),
        "rendered: placed for the 100px subject"
    );
    assert_eq!(doc.value(element, "opacity"), "1");
}

#[test]
fn range_keyframes_run_in_every_direction() {
    let keyframes = "
        @keyframes lopsided {
            entry 0% { opacity: 0; }
            entry 100% { opacity: 0.6; }
            exit 100% { opacity: 1; } }
        @keyframes stepped {
            entry 0% { opacity: 0; animation-timing-function: steps(2, jump-end); }
            entry 100% { opacity: 1; } }";
    for (animation, offset, expected) in [
        ("lopsided linear both", 700.0, "0.9"),
        ("lopsided linear both reverse", 300.0, "0.9"),
        ("lopsided linear both 2 alternate", 300.0, "0.6"),
        ("lopsided linear both 2 alternate", 600.0, "0.8"),
        ("stepped linear both; animation-range: entry", 250.0, "0"),
        ("stepped linear both; animation-range: entry", 350.0, "0.5"),
        (
            "stepped linear both reverse; animation-range: entry",
            250.0,
            "0.75",
        ),
    ] {
        let (mut doc, container, subject) = range_page(keyframes, animation);
        assert_eq!(
            opacity_at(&mut doc, container, subject, offset),
            expected,
            "{animation} at {offset}"
        );
    }
}

#[test]
fn range_keyframes_are_ignored_on_inactive_timelines_and_empty_ranges() {
    let keyframes = "@keyframes partly { entry 0% { opacity: 0.2; } to { width: 100px; } }";
    let (mut doc, container, subject) = range_page(keyframes, "partly linear both");
    doc.set_inline(
        subject,
        "animation: partly linear both; animation-timeline: --missing",
    );
    doc.flush();
    assert_eq!(opacity_at(&mut doc, container, subject, 300.0), "1");
    assert_eq!(doc.value(subject, "width"), "200px", "inactive: idle");
    assert_eq!(sampled_opacity(&doc, subject, "partly", 0.0), None);
    assert!(!doc.dom.get(subject).expect("live").animates_opacity());

    doc.set_inline(
        subject,
        "animation: partly linear both; animation-timeline: --foo;
         animation-range: contain 50% contain 50%",
    );
    doc.flush();
    assert_eq!(opacity_at(&mut doc, container, subject, 700.0), "1");
    assert_eq!(
        doc.value(subject, "width"),
        "100px",
        "the `to` keyframe fills"
    );
    assert_eq!(sampled_opacity(&doc, subject, "partly", 0.0), None);
    assert!(!doc.dom.get(subject).expect("live").animates_opacity());
}

#[test]
fn a_subject_without_a_box_and_back_places_range_keyframes_again() {
    let (mut doc, container, subject) = range_page("", "fade-in-out linear both");
    assert_eq!(opacity_at(&mut doc, container, subject, 300.0), "0.5");
    assert_eq!(
        sampled_opacity(&doc, subject, "fade-in-out", 1.0 / 6.0),
        Some(0.5)
    );
    let animation = "animation: fade-in-out linear both; animation-timeline: --foo";
    doc.set_inline(subject, &format!("display: none; {animation}"));
    doc.flush();
    assert_eq!(
        sampled_opacity(&doc, subject, "fade-in-out", 1.0 / 6.0),
        None,
        "no box: ignored"
    );
    doc.set_inline(subject, &format!("height: 100px; {animation}"));
    doc.flush();
    assert_eq!(
        sampled_opacity(&doc, subject, "fade-in-out", 1.0 / 6.0),
        Some(0.833_333_3),
        "placed for the 100px subject"
    );
    assert_eq!(doc.value(subject, "opacity"), "1");
}
