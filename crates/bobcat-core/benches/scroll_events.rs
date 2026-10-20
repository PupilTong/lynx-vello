//! One frame of scroll posts and the `scroll`/`scrollend` delivery they owe,
//! `CodSpeed`-compatible.
//!
//! A page of `CONTAINERS` scroll containers, every one with a scroll range.
//! Each case times one frame: the painter's posts adopted into the document,
//! which queues the events each container owes, then the delivery of what was
//! queued, as the page's epilogue posts it
//! ([`ScriptHarness::scroll_frame`]). What varies is where the one listener is
//! and which containers move.
//!
//! Whether a container's event is queued is decided by name over the whole
//! document, not per container: once any registration of the name exists,
//! or a live element whose component observes it, every container that
//! moves queues its event, and the delivery dispatches each one — at a
//! container with no registration on its path, that is the path built and
//! walked through the components without a realm call. The `scroll-view`
//! component observes `scroll` (its threshold events), so a page of
//! `scroll-view`s queues even with no listener anywhere; the floor is
//! therefore a page of `view`s with `overflow-y: scroll`, scroll containers
//! that no component observes. So `scroll_nobody_listens` is the
//! cost with nothing queued, `scroll_components_observe` the cost the
//! `scroll-view` component adds to that with no listener at all, the two
//! `*_one_listened_elsewhere` cases the cost of a frame's events dispatched
//! to no realm (`scroll` while moving, `scrollend` at rest), and
//! `scroll_the_listened_container` one event that does call the realm.
//!
//! The realm boots once per case, outside the timed region. Every timed frame
//! moves its containers: the posts alternate between two offsets.
//!
//! Every frame also commits, as the page's epilogue does after a post that
//! moved a container: the adopted offsets leave the document needing a
//! render, and the delivery waits for the commit. That commit is common to
//! every case, and `scroll_nobody_listens` is little more than it and the
//! adoption, so what a listener's placement adds is read against that case.

use bobcat_core::bench_support::{ScriptHarness, ScrollPost};

fn main() {
    divan::main();
}

/// Containers on the page. A screenful of short scrollers, each one a
/// separate scroll container with its own offset.
const CONTAINERS: usize = 20;

/// The two offsets the frames alternate between, both inside every
/// container's scroll range (`ROWS` rows of 40px in a 30px box).
const OFFSETS: [f32; 2] = [20.0, 40.0];

/// The page every case boots: `CONTAINERS` containers of `tag` in a column,
/// each 30px tall and holding three 40px rows, laid out by the boot's flush.
/// `style` is extra inline style for each container ([`VIEW_STYLE`] or
/// nothing); `listener` is a module body run before the flush, with
/// `scrollers` in scope.
///
/// The listener is a main-thread function, which files the same path
/// registration a `bindscroll` does and keeps the dispatch on this thread:
/// the harness has no background thread to publish a `bindscroll` to.
fn page_source(tag: &str, style: &str, listener: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          __SetInlineStyles(page, {{ display: 'flex', flexDirection: 'column' }});
          const scrollers = [];
          for (let i = 0; i < {CONTAINERS}; i += 1) {{
            const scroller = __CreateElement('{tag}', 0);
            __SetInlineStyles(scroller, {{ {style} width: '100%', height: '30px', flexShrink: '0' }});
            for (let j = 0; j < 3; j += 1) {{
              const row = __CreateView(0);
              __SetInlineStyles(row, {{ width: '100%', height: '40px', flexShrink: '0' }});
              __AppendElement(scroller, row);
            }}
            __AppendElement(page, scroller);
            scrollers.push(scroller);
          }}
          globalThis.held = [page, scrollers];
          {listener}
          __FlushElementTree();
        }};
        "
    )
}

/// A `scroll` listener on the first container.
const LISTEN_SCROLL: &str = "__AddEventListener(scrollers[0], 'scroll', () => {}, {});";

/// A `scrollend` listener on the first container.
const LISTEN_SCROLL_END: &str = "__AddEventListener(scrollers[0], 'scrollend', () => {}, {});";

/// The tag whose component observes `scroll`.
const SCROLL_VIEW: &str = "scroll-view";

/// The tag no component observes, made a scroll container by
/// [`VIEW_STYLE`] (a `view` defaults to visible overflow).
const VIEW: &str = "view";

/// The inline style that makes a [`VIEW`] container scroll on y, as the UA
/// sheet makes a `scroll-view`.
const VIEW_STYLE: &str = "overflowY: 'scroll',";

/// Boots a page of `tag` containers and answers the harness with the two
/// frames' posts for the containers `moving` names, at rest when `rest` is
/// set.
fn setup(
    tag: &str,
    listener: &str,
    moving: std::ops::Range<usize>,
    rest: bool,
) -> (ScriptHarness, [Vec<ScrollPost>; 2]) {
    let mut harness = ScriptHarness::new();
    let style = if tag == VIEW { VIEW_STYLE } else { "" };
    harness.boot(&page_source(tag, style, listener));
    let page = harness.page();
    assert_eq!(harness.child_count(page), CONTAINERS);
    let containers: Vec<u64> = (0..CONTAINERS)
        .map(|index| harness.child(page, index).expect("a container"))
        .collect();
    let posts = OFFSETS.map(|y| {
        containers[moving.clone()]
            .iter()
            .map(|&node| ScrollPost { node, y, rest })
            .collect()
    });
    (harness, posts)
}

/// Times one frame per iteration, alternating between the two offsets so
/// every frame moves its containers. A case that checks its setup with one
/// frame posts the second offset there, so the first timed frame posts the
/// first.
fn run(bencher: divan::Bencher, mut harness: ScriptHarness, posts: &[Vec<ScrollPost>; 2]) {
    let mut frame = 0usize;
    bencher.bench_local(|| {
        let posts = &posts[frame % 2];
        frame += 1;
        harness.scroll_frame(posts)
    });
}

/// Every container moves and nothing anywhere listens for `scroll` or
/// `scrollend`: `view`s, which no component observes, so nothing is queued.
#[divan::bench]
fn scroll_nobody_listens(bencher: divan::Bencher) {
    let (mut harness, posts) = setup(VIEW, "", 0..CONTAINERS, false);
    assert!(
        !harness.scroll_frame(&posts[1]),
        "nothing listens and no component observes: no delivery"
    );
    run(bencher, harness, &posts);
}

/// Every container moves and nothing listens, but the containers are
/// `scroll-view`s, whose component observes `scroll`: every one's `scroll`
/// is queued and walked to the component, which finds no listener for its
/// threshold events, and no realm is called.
#[divan::bench]
fn scroll_components_observe(bencher: divan::Bencher) {
    let (mut harness, posts) = setup(SCROLL_VIEW, "", 0..CONTAINERS, false);
    assert!(
        harness.scroll_frame(&posts[1]),
        "the scroll-view component observes `scroll`: a delivery to no realm"
    );
    run(bencher, harness, &posts);
}

/// The first container listens for `scroll`; every other container moves.
#[divan::bench]
fn scroll_one_listened_elsewhere(bencher: divan::Bencher) {
    let (harness, posts) = setup(SCROLL_VIEW, LISTEN_SCROLL, 1..CONTAINERS, false);
    run(bencher, harness, &posts);
}

/// The first container listens for `scroll`, and it alone moves: one
/// dispatch, which calls the realm.
#[divan::bench]
fn scroll_the_listened_container(bencher: divan::Bencher) {
    let (mut harness, posts) = setup(SCROLL_VIEW, LISTEN_SCROLL, 0..1, false);
    assert!(
        harness.scroll_frame(&posts[1]),
        "the listened container's `scroll` is delivered"
    );
    run(bencher, harness, &posts);
}

/// The first container listens for `scrollend`; every other container moves
/// and comes to rest in the same post, so each one's `scrollend` is the
/// event in question and `scroll` is heard nowhere.
#[divan::bench]
fn scrollend_one_listened_elsewhere(bencher: divan::Bencher) {
    let (harness, posts) = setup(SCROLL_VIEW, LISTEN_SCROLL_END, 1..CONTAINERS, true);
    run(bencher, harness, &posts);
}
