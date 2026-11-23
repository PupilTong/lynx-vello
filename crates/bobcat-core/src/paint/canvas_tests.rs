//! A placeholder `canvas` end to end: a real view and an offscreen painter,
//! canvases drawn on the GPU and committed from the test thread and from
//! another, read back as pixels.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use terminus::link::NodeId;
use terminus::link::html::canvas::{
    CanvasFrames, ContextMode, OffscreenCanvas, Placeholder, Style, context_mode,
    transfer_control_to_offscreen,
};
use terminus::link::peniko::Color;

use super::CanvasPresenter;
use crate::test_support::{TestEngine, TestViewSpec};
use crate::view::{EventRequester, Screenshot};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// Counts the wakes a commit asks for.
#[derive(Default)]
struct Wakes(AtomicUsize);

impl EventRequester for Wakes {
    fn request_event(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A page of one `canvas` with `attributes` (`[name, value]` pairs) and
/// inline `style`, on a 40 × 30 offscreen view.
fn page(attributes: &str, style: &str) -> TestEngine {
    TestViewSpec::new(&format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const canvas = __CreateElement('canvas', 0);
          for (const [name, value] of {attributes}) __SetAttribute(canvas, name, value);
          __SetInlineStyles(canvas, '{style}');
          __AppendElement(page, canvas);
        }};
        "
    ))
    .offscreen(40.0, 30.0)
    .boot()
}

/// The page's canvas.
fn canvas_of(engine: &mut TestEngine) -> NodeId {
    engine
        .probe_document(|document| {
            document
                .document_element()
                .children()
                .find(|node| node.tag_name() == Some("canvas"))
                .map(terminus::link::Node::id)
        })
        .flatten()
        .expect("the page has a canvas")
}

/// The canvas's border box in the layout.
fn rect(engine: &mut TestEngine, canvas: NodeId) -> (f32, f32) {
    let rect = engine
        .probe_document(move |document| {
            document.layout();
            document.bounding_client_rect(canvas)
        })
        .flatten()
        .expect("the canvas has a box");
    (rect.size.width, rect.size.height)
}

/// Transfers the canvas's control on `bobcat-main` and registers its link
/// for the view's painter, waking `wakes`.
fn transfer(
    engine: &mut TestEngine,
    canvas: NodeId,
    wakes: Arc<Wakes>,
) -> (u32, u32, CanvasPresenter) {
    let Placeholder {
        width,
        height,
        frames,
    } = engine
        .probe_document(move |document| transfer_control_to_offscreen(document, canvas))
        .expect("the view's task answers probes")
        .expect("a canvas in mode none");
    (width, height, register(engine, frames, wakes))
}

fn register(engine: &TestEngine, frames: CanvasFrames, wakes: Arc<Wakes>) -> CanvasPresenter {
    let seat = engine.view.seat();
    CanvasPresenter::register(
        &seat.canvases,
        frames,
        wakes,
        Some(seat.commands.uncounted()),
    )
}

/// A `width` × `height` canvas whose left half is `left` and right half
/// `right`, drawn on the GPU.
fn frame(width: u32, height: u32, left: [u8; 4], right: [u8; 4]) -> OffscreenCanvas {
    let mut canvas = OffscreenCanvas::new(width, height);
    let half = f64::from(width / 2);
    for (colour, x, w) in [(left, 0.0, half), (right, half, f64::from(width) - half)] {
        let [r, g, b, a] = colour;
        canvas
            .context_2d()
            .set_fill_style(Style::Color(Color::from_rgba8(r, g, b, a)));
        canvas.context_2d().fill_rect(x, 0.0, w, f64::from(height));
    }
    canvas
}

fn solid(width: u32, height: u32, colour: [u8; 4]) -> OffscreenCanvas {
    frame(width, height, colour, colour)
}

fn pixel(shot: &Screenshot, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * shot.size.width + x) * 4) as usize;
    [
        shot.pixels[at],
        shot.pixels[at + 1],
        shot.pixels[at + 2],
        shot.pixels[at + 3],
    ]
}

fn capture(engine: &mut TestEngine) -> Screenshot {
    assert!(
        !engine.painter.awaits_pictures(),
        "a canvas is never waited for"
    );
    engine.capture().expect("the page captures")
}

fn commit_id(engine: &mut TestEngine) -> u64 {
    engine
        .published_frame()
        .expect("a committed frame")
        .commit_id()
}

/// HTML's defaults and the attributes give the canvas's natural size, and
/// so its box when CSS names none.
#[test]
fn a_canvas_is_sized_by_its_attributes() {
    let mut engine = page("[]", "align-self:flex-start");
    let canvas = canvas_of(&mut engine);
    assert_eq!(rect(&mut engine, canvas), (300.0, 150.0));

    let mut engine = page(
        "[['width', '20'], ['height', '10']]",
        "align-self:flex-start",
    );
    let canvas = canvas_of(&mut engine);
    assert_eq!(rect(&mut engine, canvas), (20.0, 10.0));
}

/// Committed canvases reach the screen with no document commit, stretched
/// to the CSS size: one colour, two colours, a canvas of another size, and
/// one committed from another thread.
#[test]
fn committed_canvases_draw_stretched_and_without_a_document_commit() {
    let mut engine = page(
        "[['width', '4'], ['height', '2']]",
        "width:40px;height:30px;align-self:flex-start",
    );
    let canvas = canvas_of(&mut engine);
    let wakes = Arc::new(Wakes::default());
    let (width, height, presenter) = transfer(&mut engine, canvas, Arc::clone(&wakes));
    assert_eq!((width, height), (4, 2));
    let commit = commit_id(&mut engine);

    presenter.commit(&mut solid(4, 2, RED));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1, "the first commit wakes");
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 20, 15), RED);

    // One canvas drawn on after its commit: the placeholder shows the
    // commit, and the next commit shows the rest.
    let mut drawn = frame(4, 2, RED, BLUE);
    presenter.commit(&mut drawn);
    drawn
        .context_2d()
        .set_fill_style(Style::Color(Color::from_rgba8(0, 255, 0, 255)));
    drawn.context_2d().fill_rect(0.0, 0.0, 2.0, 2.0);
    drawn.flush();
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 5, 15), RED, "the left half stretched");
    assert_eq!(pixel(&shot, 35, 15), BLUE, "the right half stretched");
    assert_eq!(
        commit_id(&mut engine),
        commit,
        "no document commit drew either"
    );
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        2,
        "one wake per commit described"
    );
    presenter.commit(&mut drawn);
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 5, 15), GREEN, "the later draw, committed");

    presenter.commit(&mut solid(8, 8, GREEN));
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 5, 5), GREEN, "another size, the same box");
    assert_eq!(rect(&mut engine, canvas), (40.0, 30.0));

    let other = presenter.clone();
    std::thread::spawn(move || other.commit(&mut solid(2, 2, BLUE)))
        .join()
        .expect("the owner's thread commits");
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 20, 15), BLUE, "from another thread");
}

/// A commit of a new size becomes the placeholder's natural size once
/// `bobcat-main` has applied it, with no other command asking.
#[test]
fn a_commit_of_a_new_size_resizes_the_placeholder() {
    let mut engine = page("[]", "align-self:flex-start");
    let canvas = canvas_of(&mut engine);
    let (_, _, presenter) = transfer(&mut engine, canvas, Arc::default());
    let before = commit_id(&mut engine);
    presenter.commit(&mut solid(20, 10, RED));
    // The probe lays the document out itself; the frame drawn is the one a
    // document commit after the resize publishes.
    let deadline = Instant::now() + Duration::from_secs(20);
    while rect(&mut engine, canvas) != (20.0, 10.0) || commit_id(&mut engine) == before {
        assert!(Instant::now() < deadline, "the resize was not applied");
        engine.pump();
        std::thread::yield_now();
    }
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 10, 5), RED);
    assert_ne!(pixel(&shot, 30, 5), RED, "outside the resized box");
}

/// Under `filter: blur()` a new commit is baked again.
#[test]
fn a_new_commit_refreshes_a_filter_bake() {
    let mut engine = page(
        "[]",
        "width:40px;height:30px;align-self:flex-start;filter:blur(1px)",
    );
    let canvas = canvas_of(&mut engine);
    let (_, _, presenter) = transfer(&mut engine, canvas, Arc::default());
    presenter.commit(&mut solid(4, 4, RED));
    assert_eq!(pixel(&capture(&mut engine), 20, 15), RED);
    presenter.commit(&mut solid(4, 4, BLUE));
    assert_eq!(pixel(&capture(&mut engine), 20, 15), BLUE);
}

/// A canvas nothing was committed to — placeholder or not — draws nothing
/// and is not waited for.
#[test]
fn a_canvas_never_committed_draws_nothing() {
    let mut engine = page(
        "[]",
        "width:40px;height:30px;align-self:flex-start;background-color:#00ff00",
    );
    let shot = capture(&mut engine);
    assert_eq!(
        pixel(&shot, 20, 15),
        GREEN,
        "mode none shows its background"
    );

    let canvas = canvas_of(&mut engine);
    let (_, _, _presenter) = transfer(&mut engine, canvas, Arc::default());
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 20, 15), GREEN, "a placeholder with no commit");
}

/// The picture's texture is the link's: however many images other pictures
/// upload past the bitmap budget, the placeholder keeps drawing its commit.
#[test]
fn a_committed_texture_outlives_the_bitmap_budget() {
    let mut engine = page("[]", "width:40px;height:30px;align-self:flex-start");
    let canvas = canvas_of(&mut engine);
    let (_, _, presenter) = transfer(&mut engine, canvas, Arc::default());
    presenter.commit(&mut solid(4, 4, RED));
    assert_eq!(pixel(&capture(&mut engine), 20, 15), RED);
    // 160 MiB of bitmaps drawn by canvases, past the 128 MiB kept.
    for _ in 0..10 {
        let pixels = vec![255; 2048 * 2048 * 4];
        let image = terminus::link::html::canvas::Image::from_premultiplied(2048, 2048, pixels)
            .expect("an image the device takes");
        let mut other = OffscreenCanvas::new(1, 1);
        other
            .context_2d()
            .draw_image(&image, 0.0, 0.0)
            .expect("a drawable image");
        other.flush();
    }
    assert_eq!(pixel(&capture(&mut engine), 20, 15), RED);
}

/// Once the element is freed, a commit does nothing and wakes nobody.
#[test]
fn a_freed_canvas_takes_no_commits() {
    let mut engine = page("[]", "width:40px;height:30px;align-self:flex-start");
    let canvas = canvas_of(&mut engine);
    let wakes = Arc::new(Wakes::default());
    let (_, _, presenter) = transfer(&mut engine, canvas, Arc::clone(&wakes));
    engine
        .probe_document(move |document| {
            document.drop_element(canvas);
        })
        .expect("the view's task answers probes");
    presenter.commit(&mut solid(4, 4, RED));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    let shot = capture(&mut engine);
    assert_ne!(pixel(&shot, 20, 15), RED);
}

/// What a test's view has said: its console messages, and whether boot
/// finished.
#[derive(Default)]
struct Log {
    messages: Vec<String>,
    booted: bool,
}

impl Log {
    fn has(&self, message: &str) -> bool {
        self.messages.iter().any(|seen| seen == message)
    }

    /// Pumps `engine` until `done` holds of what it said; any script
    /// failure fails the test.
    fn until(&mut self, engine: &mut TestEngine, done: impl Fn(&Self) -> bool) {
        use crate::view::EngineEvent;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(self) {
            for event in engine.pump() {
                match event {
                    EngineEvent::ConsoleMessage { message, .. } => self.messages.push(message),
                    EngineEvent::ScriptFinished => self.booted = true,
                    EngineEvent::ScriptRunError(error)
                    | EngineEvent::TimerFailed(error)
                    | EngineEvent::ListenerFailed(error) => panic!("a script failed: {error}"),
                    EngineEvent::ScriptReported { message, .. } => panic!("reported: {message}"),
                    EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                    EngineEvent::Panicked(error) => panic!("the engine panicked: {error}"),
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline, "never came: {:?}", self.messages);
            std::thread::yield_now();
        }
    }
}

/// A page whose `renderPage` makes one `canvas` with inline `style`, and
/// then runs `script` with `page`, `canvas` and the canvas members as `H`
/// in scope, on a 40 × 30 offscreen view, booted. `transfer()` in `script`
/// is `canvasTransferControl` on the canvas, answering the new canvas's
/// `[id, width, height]` or the error name.
fn scripted(style: &str, script: &str) -> (TestEngine, Log) {
    let mut engine = TestViewSpec::new(&format!(
        r"
        import * as H from 'bobcat-internal:host';
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const canvas = __CreateElement('canvas', 0);
          __SetInlineStyles(canvas, '{style}');
          __AppendElement(page, canvas);
          const transfer = (element = canvas) => {{
            const answer = H.canvasTransferControl(__GetElementUniqueID(element));
            return typeof answer === 'string' ? answer : Array.from(new Float64Array(answer));
          }};
          {script}
        }};
        "
    ))
    .offscreen(40.0, 30.0)
    .create(Arc::new(crate::view::NoWakeup));
    let mut log = Log::default();
    log.until(&mut engine, |log| log.booted);
    (engine, log)
}

/// Captures until `(x, y)` is `colour`.
fn capture_until(engine: &mut TestEngine, x: u32, y: u32, colour: [u8; 4]) -> Screenshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let shot = capture(engine);
        if pixel(&shot, x, y) == colour {
            return shot;
        }
        assert!(
            Instant::now() < deadline,
            "({x}, {y}) is {:?}",
            pixel(&shot, x, y)
        );
        engine.pump();
        std::thread::yield_now();
    }
}

/// `canvasTransferControl` binds a canvas entry of the placeholder's size,
/// drawn in the MTS realm and committed at the end of the entry; a second
/// transfer, and one of an element of another kind, answer
/// `InvalidStateError`. A draw in a later timer entry reaches the screen
/// with no document commit.
#[test]
fn an_mts_transfer_draws_into_the_canvas_box() {
    let (mut engine, mut log) = scripted(
        "width:40px;height:30px;align-self:flex-start",
        r"
          __SetAttribute(canvas, 'width', '4');
          __SetAttribute(canvas, 'height', '2');
          const [id, width, height] = transfer();
          console.log(`transfer ${id >= 1} ${width}x${height}`);
          console.log(`again ${transfer()}`);
          console.log(`page ${transfer(page)}`);
          H.canvasSetColor(id, 0, 'red');
          H.canvasFillRect(id, 0, 0, 4, 2);
          __AddEventListener(canvas, 'touchstart', () => {
            setTimeout(() => {
              H.canvasSetColor(id, 0, 'blue');
              H.canvasFillRect(id, 2, 0, 2, 2);
              console.log('blue');
            }, 0);
          }, {});
          __FlushElementTree();
        ",
    );
    for said in [
        "transfer true 4x2",
        "again InvalidStateError",
        "page InvalidStateError",
    ] {
        assert!(log.has(said), "{said}: {:?}", log.messages);
    }
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 5, 15), RED, "the boot entry's draw");
    assert_eq!(pixel(&shot, 35, 15), RED);

    let commit = commit_id(&mut engine);
    engine.dispatch_input(terminus::link::input::InputEvent::pointer(
        terminus::link::Point2D::new(20.0, 15.0),
        1,
        terminus::link::input::PointerKind::Touch,
        terminus::link::input::PointerPhase::Down,
    ));
    log.until(&mut engine, |log| log.has("blue"));
    let shot = capture_until(&mut engine, 35, 15, BLUE);
    assert_eq!(pixel(&shot, 5, 15), RED, "the left half kept");
    assert_eq!(
        commit_id(&mut engine),
        commit,
        "no document commit drew the timer's draw"
    );
}

/// A card's `__InvokeUIMethod(canvas, 'transferControlToOffscreen', …)`
/// takes the generic UI-method path: no element kind and no base method
/// answers that name, so the answer is code 3 with no data, and the canvas
/// stays in context mode none.
#[test]
fn a_cards_invoke_of_transfer_control_to_offscreen_transfers_nothing() {
    let (mut engine, log) = scripted(
        "width:40px;height:30px;align-self:flex-start",
        r"
          __InvokeUIMethod(canvas, 'transferControlToOffscreen', {}, (result) => {
            console.log(`invoke ${result.code} ${result.data}`);
          });
          __FlushElementTree();
        ",
    );
    assert!(
        log.has("invoke 3 undefined"),
        "the answer: {:?}",
        log.messages
    );
    let canvas = canvas_of(&mut engine);
    assert_eq!(
        engine.probe_document(move |document| context_mode(document, canvas)),
        Some(Some(ContextMode::None))
    );
}

/// Setting the canvas's size resizes its bitmap, and the commit carries the
/// new size to the placeholder's natural size.
#[test]
fn resizing_the_offscreen_canvas_resizes_the_placeholder() {
    let (mut engine, _) = scripted(
        "align-self:flex-start",
        r"
          const [id] = transfer();
          H.canvasSetSize(id, 20, 10);
          H.canvasSetColor(id, 0, 'red');
          H.canvasFillRect(id, 0, 0, 20, 10);
          __FlushElementTree();
        ",
    );
    let canvas = canvas_of(&mut engine);
    let deadline = Instant::now() + Duration::from_secs(20);
    while rect(&mut engine, canvas) != (20.0, 10.0) {
        assert!(Instant::now() < deadline, "the resize was not applied");
        engine.pump();
        std::thread::yield_now();
    }
    let shot = capture_until(&mut engine, 10, 5, RED);
    assert_ne!(pixel(&shot, 30, 5), RED, "outside the resized box");
}

/// Drawing on the canvas of a removed `canvas` element does nothing the
/// page can see and throws nothing, in the entry that removed it and in a
/// later one.
#[test]
fn drawing_after_the_element_is_removed_does_nothing() {
    let (mut engine, mut log) = scripted(
        "width:40px;height:30px;align-self:flex-start",
        r"
          const [id] = transfer();
          __RemoveElement(page, canvas);
          H.canvasSetColor(id, 0, 'red');
          H.canvasFillRect(id, 0, 0, 300, 150);
          setTimeout(() => {
            H.canvasFillRect(id, 0, 0, 300, 150);
            console.log(`drawn ${new Uint8Array(H.canvasGetImageData(id, 0, 0, 1, 1))[0]}`);
          }, 0);
          __FlushElementTree();
        ",
    );
    log.until(&mut engine, |log| log.has("drawn 255"));
    let shot = capture(&mut engine);
    assert_ne!(pixel(&shot, 20, 15), RED);
}

/// A transferred canvas nothing drew on is never waited for: the page
/// captures at once, showing what is under the canvas.
#[test]
fn a_transferred_canvas_never_drawn_captures_without_waiting() {
    let (mut engine, _) = scripted(
        "width:40px;height:30px;align-self:flex-start;background-color:#00ff00",
        r"
          transfer();
          __FlushElementTree();
        ",
    );
    let shot = capture(&mut engine);
    assert_eq!(pixel(&shot, 20, 15), GREEN);
}
