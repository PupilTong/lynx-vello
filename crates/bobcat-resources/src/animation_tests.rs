//! The animated store end to end: a real `Resources` over a platform whose
//! decoder reads APNG frames with the `png` crate, so the same answers come
//! back on every operating system, with the playheads' clock driven by the
//! test.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::{ClockInstant, FrameImage, FrameImages, ImageInbox, ImageSizeHint, PictureKey};
use vant::image::animation::{ImageFrames, Loops};
use vant::image::{Bitmap, DecodeError};
use vant::{ImageDecoder, NoStore, Parts, Platform};

use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;
use crate::{NativeFetch, Resources, ResourcesConfig, ViewResources};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// An 8 × 8 APNG of one colour per frame, shown for its milliseconds,
/// played `plays` times (zero: forever).
fn apng(plays: u32, frames: &[([u8; 4], u16)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .set_animated(u32::try_from(frames.len()).expect("a few"), plays)
            .expect("animated");
        let mut writer = encoder.write_header().expect("header");
        for &(colour, millis) in frames {
            writer.set_frame_delay(millis, 1000).expect("delay");
            writer.write_image_data(&colour.repeat(64)).expect("frame");
        }
        writer.finish().expect("finish");
    }
    bytes
}

/// A still 8 × 8 PNG.
fn still_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(&WHITE.repeat(64)).expect("pixels");
    }
    bytes
}

/// Every frame of an APNG whose frames cover the whole canvas, and what
/// the file states of them.
fn read_apng(bytes: &[u8]) -> Result<(Vec<Vec<u8>>, ImageFrames), DecodeError> {
    let malformed = |error: png::DecodingError| DecodeError::Malformed(error.to_string());
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(malformed)?;
    let control = reader.info().animation_control;
    let count = control.map_or(1, |control| control.num_frames as usize);
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let (mut pixels, mut delays) = (Vec::new(), Vec::new());
    for _ in 0..count {
        reader.next_frame(&mut buffer).map_err(malformed)?;
        let delay = reader.info().frame_control.map_or(Duration::ZERO, |frame| {
            Duration::from_secs_f64(f64::from(frame.delay_num) / f64::from(frame.delay_den.max(1)))
        });
        pixels.push(buffer.clone());
        delays.push(delay);
    }
    let loops = control.map_or(Loops::ONCE, |control| Loops::from_plays(control.num_plays));
    Ok((pixels, ImageFrames::new(delays, loops)))
}

/// The decoder of these tests: APNG through the `png` crate, every frame
/// decoded by index logged, and a frame the test closes held at a gate.
#[derive(Clone, Default)]
struct PngFrames {
    decoded: Arc<Mutex<Vec<usize>>>,
    gate: Arc<(Mutex<Vec<usize>>, Condvar)>,
    /// Frames whose decode fails.
    broken: Arc<Mutex<Vec<usize>>>,
    /// How many times the store said it asks no more frames.
    forgotten: Arc<AtomicUsize>,
}

impl PngFrames {
    /// Holds every decode of `frame` until [`Self::open`].
    fn close(&self, frame: usize) {
        self.gate.0.lock().expect("gate").push(frame);
    }

    fn open(&self) {
        self.gate.0.lock().expect("gate").clear();
        self.gate.1.notify_all();
    }

    fn decoded(&self) -> Vec<usize> {
        self.decoded.lock().expect("log").clone()
    }
}

impl ImageDecoder for PngFrames {
    fn decode_blocking(
        &self,
        bytes: &[u8],
        _format: ImageFormat,
        _header: Option<ImageHeader>,
        _max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        self.frame(bytes, 0)
    }

    fn available(&self) -> Result<(), DecodeError> {
        Ok(())
    }

    fn frames_blocking(
        &self,
        bytes: &[u8],
        _format: ImageFormat,
        _header: Option<ImageHeader>,
    ) -> Result<ImageFrames, DecodeError> {
        read_apng(bytes).map(|(_, frames)| frames)
    }

    fn decode_frame_blocking(
        &self,
        bytes: &bytes::Bytes,
        _format: ImageFormat,
        _header: Option<ImageHeader>,
        index: usize,
        _max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        self.frame(bytes, index)
    }

    fn forget_frames(&self, _bytes: &bytes::Bytes) {
        self.forgotten.fetch_add(1, Ordering::SeqCst);
    }
}

impl PngFrames {
    /// Frame `index`, held at the gate while it is closed for it.
    fn frame(&self, bytes: &[u8], index: usize) -> Result<Bitmap, DecodeError> {
        {
            let (closed, opened) = &*self.gate;
            let mut closed = closed.lock().expect("gate");
            while closed.contains(&index) {
                closed = opened.wait(closed).expect("gate");
            }
        }
        self.decoded.lock().expect("log").push(index);
        if self.broken.lock().expect("broken").contains(&index) {
            return Err(DecodeError::Malformed(format!("frame {index} is broken")));
        }
        let (mut pixels, _) = read_apng(bytes)?;
        if index >= pixels.len() {
            return Err(DecodeError::Malformed(format!("no frame {index}")));
        }
        Ok(Bitmap {
            width: 8,
            height: 8,
            source_width: 8,
            source_height: 8,
            premultiplied: false,
            rgba: pixels.swap_remove(index),
        })
    }
}

struct PngPlatform(PngFrames);

impl Platform for PngPlatform {
    type Fetch = NativeFetch;
    type Store = NoStore;
    type AsyncStore = NoStore;
    type Decoder = PngFrames;

    fn into_parts(self) -> Parts<NativeFetch, NoStore, NoStore, PngFrames> {
        Parts {
            fetch: NativeFetch,
            decoder: self.0,
            bytes: None,
            artifacts: None,
        }
    }
}

/// A resource system over [`PngFrames`] whose playheads read the test's
/// clock, counting the host's wakeups.
struct Rig {
    resources: Resources<PngPlatform>,
    decoder: PngFrames,
    clock: Arc<Mutex<ClockInstant>>,
    wakeups: Arc<AtomicUsize>,
    start: ClockInstant,
}

impl Rig {
    fn new() -> Self {
        let decoder = PngFrames::default();
        let wakeups = Arc::new(AtomicUsize::new(0));
        let resources = Resources::with_platform(
            ResourcesConfig {
                worker_threads: 2,
                log_to_stderr: false,
                ..ResourcesConfig::default()
            },
            PngPlatform(decoder.clone()),
            Vec::new(),
            {
                let wakeups = Arc::clone(&wakeups);
                move || {
                    wakeups.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
        let start = ClockInstant::now();
        let clock = Arc::new(Mutex::new(start));
        resources.shared.set_clock_hook(Arc::new({
            let clock = Arc::clone(&clock);
            move || *clock.lock().expect("clock")
        }));
        Self {
            resources,
            decoder,
            clock,
            wakeups,
            start,
        }
    }

    /// The test's clock reads `at` after the start.
    fn set(&self, at: Duration) {
        *self.clock.lock().expect("clock") = self.start + at;
    }

    fn view(&self) -> (ViewResources<PngPlatform>, ImageInbox) {
        let (reports, inbox) = ImageInbox::new();
        (self.resources.for_view(reports), inbox)
    }

    /// Registers `bytes` as `source` and loads it through `view`.
    fn load(
        &self,
        view: &ViewResources<PngPlatform>,
        inbox: &ImageInbox,
        source: &str,
        bytes: Vec<u8>,
    ) {
        self.resources
            .register(source, bytes, Some("image/png"))
            .expect("register");
        bobcat_core::resource::ResourceFetcher::request_image(view, source);
        self.settle("the load never reported", || !inbox.drain().is_empty());
    }

    /// Services turns until `done` holds.
    fn settle(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            crate::images::service(&self.resources);
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(ms(2));
        }
    }

    /// Services turns until `source` holds `frames`.
    fn settle_held(&self, source: &str, frames: &[usize]) {
        self.settle(&format!("{source} never held {frames:?}"), || {
            self.resources.local.borrow().animations.held_frames(source) == frames
        });
    }

    fn position(&self, source: &str) -> (usize, u32, bool) {
        self.resources
            .local
            .borrow()
            .animations
            .position(source)
            .expect("an animated source")
    }

    fn held(&self, source: &str) -> Vec<usize> {
        self.resources.local.borrow().animations.held_frames(source)
    }
}

fn url(source: &str) -> PictureKey {
    PictureKey::Url(Arc::from(source))
}

const HINT: ImageSizeHint = ImageSizeHint::new(8, 8);

/// The colour of the top-left pixel a describe answered with.
fn colour(image: &FrameImage) -> [u8; 4] {
    let pixels = image.pixels.as_ref().expect("pixels");
    pixels.data.data()[..4].try_into().expect("four")
}

/// Six frames of 100 ms, forever.
fn six() -> Vec<u8> {
    apng(
        0,
        &[
            (RED, 100),
            (GREEN, 100),
            (BLUE, 100),
            (WHITE, 100),
            (BLACK, 100),
            (RED, 100),
        ],
    )
}

/// A load of an animated file answers its first frame with the next change
/// at its delay, and from then on each describe at a frame's due instant
/// answers the next frame, at a generation one higher; the frames ahead are
/// decoded off the painter's thread.
#[test]
fn frames_advance_at_their_delays() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    rig.load(
        &view,
        &inbox,
        "app:///a.png",
        apng(0, &[(RED, 100), (GREEN, 200), (BLUE, 300)]),
    );
    let source = url("app:///a.png");

    let first = view.describe(&source, HINT).expect("drawn");
    assert_eq!(colour(&first), RED);
    assert_eq!(first.generation, 0);
    assert_eq!(first.next_change, Some(rig.start + ms(100)));
    rig.settle_held("app:///a.png", &[1, 2]);

    rig.set(ms(99));
    assert_eq!(view.describe(&source, HINT).expect("drawn").generation, 0);
    for (at, expected, generation, next) in [
        (100, GREEN, 1, 300),
        (300, BLUE, 2, 600),
        (600, RED, 3, 700),
    ] {
        rig.set(ms(at));
        let image = view.describe(&source, HINT).expect("drawn");
        assert_eq!(
            (colour(&image), image.generation, image.next_change),
            (expected, generation, Some(rig.start + ms(next))),
            "at {at} ms"
        );
    }
    assert_eq!(rig.position("app:///a.png"), (0, 1, true));
}

/// Two draws of one URL, in two views, are one playhead: a describe from
/// either moves both, and both answer the same frame.
#[test]
fn two_draws_share_one_playhead() {
    let rig = Rig::new();
    let (first, first_inbox) = rig.view();
    let (second, second_inbox) = rig.view();
    rig.load(&first, &first_inbox, "app:///a.png", six());
    bobcat_core::resource::ResourceFetcher::request_image(&second, "app:///a.png");
    rig.settle("the second view was never told", || {
        !second_inbox.drain().is_empty()
    });
    let source = url("app:///a.png");
    first.retain(std::slice::from_ref(&source));
    second.retain(std::slice::from_ref(&source));
    assert_eq!(first.describe(&source, HINT).expect("drawn").generation, 0);
    rig.settle_held("app:///a.png", &[1, 2]);

    rig.set(ms(100));
    let moved = first.describe(&source, HINT).expect("drawn");
    let seen = second.describe(&source, HINT).expect("drawn");
    assert_eq!((moved.generation, seen.generation), (1, 1));
    assert_eq!(moved.descriptor, seen.descriptor, "one bitmap for both");

    // One view stops drawing it: the other still does, so it runs on.
    first.retain(&[]);
    rig.settle_held("app:///a.png", &[1, 2, 3]);
    rig.set(ms(200));
    assert_eq!(second.describe(&source, HINT).expect("drawn").generation, 2);
    assert_eq!(rig.position("app:///a.png"), (2, 0, true));
}

/// A picture no view draws stops: its frames are dropped, time does not
/// pass for it, and drawn again it continues from the frame it stopped on
/// for the time that frame had left.
#[test]
fn a_picture_not_drawn_stops_and_resumes_on_the_same_frame() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    rig.load(&view, &inbox, "app:///a.png", six());
    let source = url("app:///a.png");
    view.retain(std::slice::from_ref(&source));
    view.describe(&source, HINT).expect("drawn");
    rig.settle_held("app:///a.png", &[1, 2]);
    rig.set(ms(100));
    assert_eq!(view.describe(&source, HINT).expect("drawn").generation, 1);
    rig.settle_held("app:///a.png", &[1, 2, 3]);

    rig.set(ms(160));
    view.retain(&[]);
    assert_eq!(rig.position("app:///a.png"), (1, 0, false));
    assert!(
        rig.held("app:///a.png").is_empty(),
        "its frames are dropped"
    );
    let held = rig.resources.memory_used_bytes();

    rig.set(ms(60_000));
    crate::images::service(&rig.resources);
    assert_eq!(rig.position("app:///a.png"), (1, 0, false));
    assert_eq!(rig.resources.memory_used_bytes(), held);

    view.retain(std::slice::from_ref(&source));
    let resumed = view.describe(&source, HINT).expect("drawn");
    assert_eq!(resumed.generation, 1, "the same frame");
    assert_eq!(
        resumed.next_change,
        Some(rig.start + ms(60_040)),
        "the 40 ms frame 1 had left"
    );
    // Its pixels were dropped: the store answers the bitmap it was, for the
    // painter to draw from its upload.
    assert!(resumed.pixels.is_none());
    assert_eq!(
        rig.decoder.forgotten.load(Ordering::SeqCst),
        1,
        "the decoder was told its frames are not asked for while it stopped"
    );
}

/// A frame that is due and not decoded is waited for: the playhead stays,
/// the next change stays the past instant, no timer is armed for it, and the
/// decode's completion wakes the host; the frame then shows for its whole
/// delay.
#[test]
fn a_frame_not_decoded_when_due_is_waited_for() {
    let rig = Rig::new();
    rig.decoder.close(1);
    let (view, inbox) = rig.view();
    rig.load(&view, &inbox, "app:///a.png", six());
    let source = url("app:///a.png");
    view.describe(&source, HINT).expect("drawn");

    rig.set(ms(250));
    let waiting = view.describe(&source, HINT).expect("drawn");
    assert_eq!(waiting.generation, 0, "still the first frame");
    assert_eq!(waiting.next_change, Some(rig.start + ms(100)));

    let before = rig.wakeups.load(Ordering::SeqCst);
    rig.decoder.open();
    rig.settle("frame 1 never arrived", || {
        rig.held("app:///a.png").contains(&1)
    });
    assert!(
        rig.wakeups.load(Ordering::SeqCst) > before,
        "its decode woke the host"
    );
    let shown = view.describe(&source, HINT).expect("drawn");
    assert_eq!((colour(&shown), shown.generation), (GREEN, 1));
    assert_eq!(shown.next_change, Some(rig.start + ms(350)));
    assert_eq!(
        rig.decoder
            .decoded()
            .iter()
            .filter(|&&frame| frame == 1)
            .count(),
        1
    );
}

/// The timer the store arms on its executor wakes the host once per change
/// of frame: never while the playhead waits for nothing, and once for each
/// frame that comes due.
#[test]
fn the_timer_wakes_the_host_once_per_change() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    rig.load(
        &view,
        &inbox,
        "app:///a.png",
        apng(0, &[(RED, 60), (GREEN, 60), (BLUE, 60)]),
    );
    let source = url("app:///a.png");
    view.describe(&source, HINT).expect("drawn");
    // Every frame of a three-frame file stays held: nothing more decodes.
    rig.settle_held("app:///a.png", &[1, 2]);

    let mut at = 0;
    for generation in 1..=4 {
        let before = rig.wakeups.load(Ordering::SeqCst);
        // Real time passes on the executor's timer; the playheads' clock is
        // the test's, moved to the instant the timer was armed for once it
        // has woken the host.
        let deadline = Instant::now() + Duration::from_secs(30);
        while rig.wakeups.load(Ordering::SeqCst) == before {
            assert!(Instant::now() < deadline, "the timer never woke the host");
            std::thread::sleep(ms(5));
        }
        std::thread::sleep(ms(150));
        assert_eq!(
            rig.wakeups.load(Ordering::SeqCst),
            before + 1,
            "one wake for change {generation}"
        );
        at += 60;
        rig.set(ms(at));
        crate::images::service(&rig.resources);
        assert_eq!(
            view.describe(&source, HINT).expect("drawn").generation,
            generation
        );
    }
    assert!(
        rig.decoder.decoded().len() <= 3,
        "{:?}",
        rig.decoder.decoded()
    );
}

/// A file that states a count plays exactly that many loops; then the last
/// frame stays, at the same generation, with no next change and no timer.
#[test]
fn after_the_stated_loops_the_last_frame_stays() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    let fixture = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/animated/rgb-twice.png"),
    )
    .expect("the APNG fixture");
    rig.load(&view, &inbox, "app:///rgb.png", fixture);
    let source = url("app:///rgb.png");
    let mut image = view.describe(&source, HINT).expect("drawn");
    rig.settle_held("app:///rgb.png", &[1, 2]);
    let mut changes = Vec::new();
    while let Some(next) = image.next_change {
        *rig.clock.lock().expect("clock") = next;
        crate::images::service(&rig.resources);
        image = view.describe(&source, HINT).expect("drawn");
        changes.push((next.duration_since(rig.start).as_millis(), image.generation));
    }
    // Red 5 ms plays as 100 ms; green 200 ms; blue 300 ms; twice.
    assert_eq!(
        changes,
        [(100, 1), (300, 2), (600, 3), (700, 4), (900, 5), (1200, 5)]
    );
    assert_eq!(colour(&image), BLUE, "the last frame stays shown");
    assert_eq!(rig.position("app:///rgb.png"), (2, 2, true));
    // The timers armed on the way, each for at most 300 ms of real time,
    // have fired by now; a finished playhead arms none.
    std::thread::sleep(ms(600));
    crate::images::service(&rig.resources);
    let before = rig.wakeups.load(Ordering::SeqCst);
    assert!(
        view.describe(&source, HINT)
            .expect("drawn")
            .next_change
            .is_none()
    );
    std::thread::sleep(ms(400));
    crate::images::service(&rig.resources);
    assert_eq!(rig.wakeups.load(Ordering::SeqCst), before, "no timer");
}

/// While drawn, a source holds the frame shown and the two after it, and
/// the frame shown is let go once the rasterizer holds its upload; a frame
/// shown again later is decoded again.
#[test]
fn a_drawn_source_holds_a_bounded_window_of_frames() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    rig.load(&view, &inbox, "app:///a.png", six());
    let source = url("app:///a.png");
    view.retain(std::slice::from_ref(&source));
    view.describe(&source, HINT).expect("drawn");
    rig.settle_held("app:///a.png", &[1, 2]);
    for (frame, window) in [(1_u64, [1, 2, 3]), (2, [2, 3, 4]), (3, [3, 4, 5])] {
        rig.set(ms(100 * frame));
        assert!(
            view.describe(&source, HINT)
                .expect("drawn")
                .pixels
                .is_some()
        );
        rig.settle_held("app:///a.png", &window);
        view.release(std::slice::from_ref(&source));
        assert_eq!(
            rig.held("app:///a.png"),
            window[1..],
            "the frame shown is let go once uploaded"
        );
        assert!(
            view.describe(&source, HINT)
                .expect("drawn")
                .pixels
                .is_none(),
            "and described without pixels"
        );
    }
    assert!(
        rig.resources.memory_used_bytes()
            <= 3 * 8 * 8 * 4 + rig.resources.local.borrow().residency.encoded_bytes() + 8 * 8 * 4
    );
    // A frame wanted after its pixels went is decoded again.
    let decodes = rig.decoder.decoded().len();
    view.want(&source, HINT);
    rig.settle("frame 3 was never decoded again", || {
        rig.held("app:///a.png").contains(&3)
    });
    assert_eq!(rig.decoder.decoded().len(), decodes + 1);
    assert_eq!(rig.decoder.decoded().last(), Some(&3));
}

/// A frame that fails to decode stops the playhead where it is: no next
/// change, so the painter stops asking, and the failure is noted.
#[test]
fn a_frame_that_fails_to_decode_stops_the_playhead() {
    let rig = Rig::new();
    rig.decoder.broken.lock().expect("broken").push(2);
    let (view, inbox) = rig.view();
    rig.load(&view, &inbox, "app:///a.png", six());
    let source = url("app:///a.png");
    view.describe(&source, HINT).expect("drawn");
    rig.settle("frame 2 never failed", || {
        rig.resources
            .take_notes()
            .iter()
            .any(|note| note.contains("frame 2 of image `app:///a.png` could not be decoded"))
    });
    rig.set(ms(100));
    let image = view.describe(&source, HINT).expect("drawn");
    assert_eq!(image.generation, 0, "stopped on the frame it was on");
    assert_eq!(image.next_change, None, "and names no next change");
    assert_eq!(colour(&image), RED);
}

/// A still image is not an animation: generation zero, no next change, and
/// no frame decode beyond its load.
#[test]
fn a_still_image_is_untouched() {
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    rig.load(&view, &inbox, "app:///still.png", still_png());
    let image = view
        .describe(&url("app:///still.png"), HINT)
        .expect("drawn");
    assert_eq!((image.generation, image.next_change), (0, None));
    assert!(
        rig.resources
            .local
            .borrow()
            .animations
            .position("app:///still.png")
            .is_none()
    );
    assert_eq!(rig.decoder.decoded(), [0]);
}

/// A presenting painter draws two frames of one file with no commit
/// between them: the first with the commit, the second at the next
/// adoption once the playheads' clock has reached its due instant.
#[test]
fn a_painter_draws_two_frames_of_one_file_with_no_commit_between() {
    use terminus::link::painter::{DrawTarget, GesturePolicy, Painter};
    use terminus::link::{Device, Document, StylesheetOrigin};

    const SRC: &str = "app:///rgb.png";
    let rig = Rig::new();
    let (view, inbox) = rig.view();
    let fixture = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/animated/rgb-twice.png"),
    )
    .expect("the APNG fixture");
    rig.resources
        .register(SRC, fixture, Some("image/png"))
        .expect("register");

    let mut document = Document::new(Device::new(48.0, 32.0, 1.0), "page", ());
    document.add_stylesheet(
        "image { position: absolute; top: 8px; width: 16px; height: 16px; } \
         .left { left: 6px; } .right { left: 26px; }",
        StylesheetOrigin::Author,
    );
    let root = document.document_element().id();
    let mut images = Vec::new();
    for class in ["left", "right"] {
        let image = document.create_element("image", ());
        document.append_child(root, image);
        document.set_classes(image, class);
        images.push(image);
    }
    bobcat_core::resource::ResourceFetcher::request_image(&view, SRC);
    let mut events = Vec::new();
    rig.settle("the APNG never loaded", || {
        events.extend(inbox.drain());
        !events.is_empty()
    });
    let _settled = document.apply_image_events(&events);
    let natural = document.request_image(SRC).natural_size();
    for &image in &images {
        document.set_picture(image, Some((url(SRC), natural)));
    }

    let mut painter = pollster::block_on(Painter::new(
        DrawTarget::Offscreen,
        48.0,
        32.0,
        1.0,
        GesturePolicy {
            tap_slop: 50.0,
            long_press_move_slop: 8.0,
            drag_slop: 8.0,
            long_press_seconds: 0.5,
            tap_event: "tap",
            long_press_event: "longpress",
        },
    ))
    .expect("a GPU adapter is mandatory for the render tests");
    let capture = |painter: &mut Painter| {
        let screenshot = painter.capture(0.0).expect("capture");
        prime_radiant::Image::from_rgba8(
            screenshot.size.width,
            screenshot.size.height,
            screenshot.pixels,
        )
        .expect("an RGBA8 capture")
    };
    let goldens = prime_radiant::screenshots_in(env!("CARGO_MANIFEST_DIR"));

    assert!(painter.adopt(Some(document.commit()), Some(&view)));
    goldens.assert_matches(&["animated", "first-frame"], &capture(&mut painter));
    let commit = painter.frame().map(|frame| frame.commit_id());
    assert!(
        painter.next_picture_change().is_some(),
        "an animated picture names its next change"
    );
    rig.settle_held(SRC, &[1, 2]);

    // Turns before the frame is due change nothing, and wait for nothing.
    let _ = painter.take_redraw();
    for _ in 0..3 {
        crate::images::service(&rig.resources);
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&view)));
        assert!(!painter.take_redraw(), "nothing it draws changed");
        assert!(
            !painter.awaits_pictures(),
            "between frames, nothing is awaited"
        );
    }
    goldens.assert_matches(&["animated", "first-frame"], &capture(&mut painter));

    // Red's 5 ms plays as 100 ms.
    rig.set(ms(100));
    crate::images::service(&rig.resources);
    let current = painter.frame().cloned();
    assert!(painter.adopt(current, Some(&view)));
    assert!(painter.take_redraw(), "the new frame owes one frame");
    assert!(!painter.awaits_pictures(), "nothing is waited for");
    assert_eq!(
        painter.frame().map(|frame| frame.commit_id()),
        commit,
        "no commit"
    );
    goldens.assert_matches(&["animated", "second-frame"], &capture(&mut painter));
}
