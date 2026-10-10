//! The whole pipeline through the protocol's own seams, with no GPU and no
//! view: a `ViewResources` driven the way the painter drives it — request,
//! service, read — against an `ImageInbox` standing in for the document.
//!
//! Every raster image here goes through the real platform decoder, so these
//! need `ImageIO` or gdk-pixbuf and fail rather than skip without one. SVG
//! documents are parsed with the engine's own parser and reported as the
//! parsed document, and need nothing from the platform.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    DocumentKind, FrameImages, ImageEvent, ImageInbox, ImageSizeHint, VectorDocument,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};

/// A width x height PNG whose quadrants are red, green, blue and white.
fn quadrant_png(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = match (x < width / 2, y < height / 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 255],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&rgba).expect("png data");
    }
    bytes
}

struct Harness {
    resources: Resources,
    view: ViewResources,
    inbox: ImageInbox,
    wakeups: Arc<AtomicUsize>,
}

impl Harness {
    fn new(config: ResourcesConfig) -> Self {
        let wakeups = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakeups);
        let resources = Resources::new(config, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let (reports, inbox) = ImageInbox::new();
        let view = resources.for_view(reports);
        Self {
            resources,
            view,
            inbox,
            wakeups,
        }
    }

    fn quiet() -> ResourcesConfig {
        ResourcesConfig {
            log_to_stderr: false,
            ..ResourcesConfig::default()
        }
    }

    /// Drives painter turns until the inbox carries a report for `source`.
    fn settle(&self, source: &str) -> ImageEvent {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.view.service_images();
            if let Some(event) = self.inbox.drain().into_iter().find(|event| match event {
                ImageEvent::Loaded {
                    source: reported, ..
                }
                | ImageEvent::ParsedDocument {
                    source: reported, ..
                }
                | ImageEvent::Failed { source: reported } => &**reported == source,
            }) {
                return event;
            }
            assert!(Instant::now() < deadline, "`{source}` never reported");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn load(&self, source: &str) -> (u32, u32) {
        self.view.request_image(source);
        match self.settle(source) {
            ImageEvent::Loaded { width, height, .. } => (width, height),
            other => panic!(
                "`{source}` did not load as a bitmap: {other:?} {:?}",
                self.resources.take_notes()
            ),
        }
    }

    /// Requests `source` and returns the document it was parsed into.
    fn load_svg(&self, source: &str) -> Arc<VectorDocument> {
        self.view.request_image(source);
        self.parsed(source)
    }

    /// Drives turns until `source` reports, and returns the document it was
    /// parsed into.
    fn parsed(&self, source: &str) -> Arc<VectorDocument> {
        match self.settle(source) {
            ImageEvent::ParsedDocument { document, .. } => document,
            other => panic!(
                "`{source}` did not load as a parsed document: {other:?} {:?}",
                self.resources.take_notes()
            ),
        }
    }

    /// Requests `source` and asserts it fails, returning the notes.
    fn fail(&self, source: &str) -> Vec<String> {
        self.view.request_image(source);
        let event = self.settle(source);
        assert!(
            matches!(event, ImageEvent::Failed { .. }),
            "`{source}` did not fail: {event:?}"
        );
        self.resources.take_notes()
    }

    /// Drives turns until the resident bitmap for `source` has `size`.
    fn settle_resident(&self, source: &str, size: (u32, u32)) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.view.service_images();
            if self.resources.resident_size(source) == Some(size) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "`{source}` never became {size:?}: {:?}",
                self.resources.resident_size(source)
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn a_registered_png_loads_reports_its_size_and_reads_back_at_the_drawn_size() {
    let harness = Harness::new(Harness::quiet());
    harness
        .resources
        .register("app:///checker.png", quadrant_png(64, 32), None)
        .expect("register");

    assert_eq!(harness.load("app:///checker.png"), (64, 32));
    assert!(
        harness.wakeups.load(Ordering::SeqCst) >= 1,
        "a completion wakes the host"
    );
    assert!(harness.resources.is_resident("app:///checker.png"));

    let image = harness
        .view
        .read("app:///checker.png", ImageSizeHint::new(64, 32))
        .expect("a loaded image reads");
    assert_eq!((image.width, image.height), (64, 32));
    assert_eq!(
        &image.data.as_ref()[..4],
        &[255, 0, 0, 255],
        "top-left is red"
    );

    // Asking again answers from what is known, without a second load.
    let (reports, inbox) = ImageInbox::new();
    let second = harness.resources.for_view(reports);
    second.request_image("app:///checker.png");
    assert!(matches!(
        inbox.drain().as_slice(),
        [ImageEvent::Loaded {
            width: 64,
            height: 32,
            ..
        }]
    ));
}

#[test]
fn a_draw_far_smaller_than_the_bitmap_refines_it_to_the_drawn_size() {
    let harness = Harness::new(ResourcesConfig {
        initial_decode_bound: 4096,
        downsample_ratio: 2.0,
        ..Harness::quiet()
    });
    harness
        .resources
        .register("app:///big.png", quadrant_png(1024, 512), None)
        .expect("register");
    assert_eq!(harness.load("app:///big.png"), (1024, 512));
    assert_eq!(
        harness.resources.resident_size("app:///big.png"),
        Some((1024, 512))
    );

    // The first read answers with what is resident and starts the refinement.
    let first = harness
        .view
        .read("app:///big.png", ImageSizeHint::new(100, 100))
        .expect("reads");
    assert_eq!((first.width, first.height), (1024, 512));
    harness.settle_resident("app:///big.png", (100, 50));

    let refined = harness
        .view
        .read("app:///big.png", ImageSizeHint::new(100, 100))
        .expect("reads");
    assert_eq!(
        (refined.width, refined.height),
        (100, 50),
        "decoded for the draw"
    );
    assert_eq!(&refined.data.as_ref()[..4], &[255, 0, 0, 255]);

    // Drawn larger again: the image has more to give, so it refines back up.
    let _ = harness
        .view
        .read("app:///big.png", ImageSizeHint::new(400, 400));
    harness.settle_resident("app:///big.png", (400, 200));
}

#[test]
fn an_evicted_bitmap_is_restored_inside_the_read() {
    let harness = Harness::new(Harness::quiet());
    harness
        .resources
        .register("app:///a.png", quadrant_png(32, 32), None)
        .expect("register");
    harness
        .resources
        .register("app:///b.png", quadrant_png(32, 32), None)
        .expect("register");
    harness.load("app:///a.png");
    harness.load("app:///b.png");
    let used = harness.resources.memory_used_bytes();
    assert!(used >= 2 * 32 * 32 * 4, "both bitmaps are resident: {used}");

    // A budget for one bitmap, with `b` as the working set: `a` is evicted.
    harness.view.retain(&[Arc::from("app:///b.png")]);
    harness.resources.set_memory_budget_bytes(32 * 32 * 4 + 16);
    assert!(!harness.resources.is_resident("app:///a.png"));
    assert!(harness.resources.is_resident("app:///b.png"));

    let restored = harness
        .view
        .read("app:///a.png", ImageSizeHint::new(16, 16))
        .expect("a reported load never misses");
    assert_eq!(
        (restored.width, restored.height),
        (16, 16),
        "restored for the draw"
    );
    assert!(harness.resources.is_resident("app:///a.png"));
}

#[test]
fn data_urls_and_non_images_and_unknown_schemes_report_precisely() {
    let harness = Harness::new(Harness::quiet());
    let png = quadrant_png(8, 8);
    let data_url = format!("data:image/png;base64,{}", base64_encode(&png));
    assert_eq!(harness.load(&data_url), (8, 8));

    harness
        .resources
        .register("app:///text.txt", b"hello".to_vec(), Some("text/plain"))
        .expect("register");
    harness.view.request_image("app:///text.txt");
    assert!(matches!(
        harness.settle("app:///text.txt"),
        ImageEvent::Failed { .. }
    ));
    assert!(
        harness
            .resources
            .take_notes()
            .iter()
            .any(|note| note.contains("not an image"))
    );

    harness.view.request_image("gopher://nowhere/x.png");
    assert!(matches!(
        harness.settle("gopher://nowhere/x.png"),
        ImageEvent::Failed { .. }
    ));
    assert!(
        harness
            .view
            .read("gopher://nowhere/x.png", ImageSizeHint::UNBOUNDED)
            .is_none()
    );
}

#[test]
fn every_view_of_the_shared_system_sees_the_same_registrations_and_state() {
    let harness = Harness::new(Harness::quiet());
    harness
        .resources
        .register("app:///shared.png", quadrant_png(16, 16), None)
        .expect("register");
    let builder = harness.resources.builder();
    let (reports, inbox) = ImageInbox::new();
    let other = builder(reports);
    other.request_image("app:///shared.png");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        other.service_images();
        if !inbox.drain().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(harness.resources.knows_image("app:///shared.png"));
    assert!(
        Rc::new(harness.view)
            .read("app:///shared.png", ImageSizeHint::UNBOUNDED)
            .is_some()
    );
}

/// A file under the platform's temp directory, removed when it goes.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> Self {
        let path =
            std::env::temp_dir().join(format!("bobcat-resources-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write the temp file");
        Self(path)
    }

    fn url(&self) -> url::Url {
        url::Url::from_file_path(&self.0).expect("a file URL")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A `file:`-backed image keeps no encoded bytes — the file is the tier that
/// can hand them back — so an evicted one is re-fetched *and* re-decoded on
/// the painter's thread, inside the read. This test's thread is inside
/// another runtime's `block_on`, which the capture server's is too.
#[tokio::test(flavor = "current_thread")]
async fn an_evicted_file_backed_image_is_re_fetched_and_decoded_inside_the_read() {
    let file = TempFile::new("restore.png", &quadrant_png(64, 64));
    let harness = Harness::new(Harness::quiet());
    let source = file.url().to_string();
    assert_eq!(harness.load(&source), (64, 64));
    assert!(
        harness.resources.memory_used_bytes() >= 64 * 64 * 4,
        "the bitmap is resident"
    );

    // Nothing is pinned, so a zero budget evicts it; the file kept no bytes.
    harness.view.retain(&[]);
    harness.resources.set_memory_budget_bytes(0);
    assert!(!harness.resources.is_resident(&source));
    assert_eq!(
        harness.resources.memory_used_bytes(),
        0,
        "a restorable image keeps no encoded bytes either"
    );

    let restored = harness
        .view
        .read(&source, ImageSizeHint::new(32, 32))
        .expect("a reported load never misses");
    assert_eq!((restored.width, restored.height), (32, 32));
    assert_eq!(&restored.data.as_ref()[..4], &[255, 0, 0, 255]);
    assert!(harness.resources.take_notes().is_empty());
}

/// An SVG document whose root carries `attributes`, with one filled rect.
fn svg(attributes: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" {attributes}><rect x="1" y="1" width="6" height="4" fill="#dc2626"/></svg>"##
    )
}

/// What the engine's parser makes of `svg`, as text: a reported document
/// prints the same exactly when it was parsed from the same bytes. The
/// document has no public members, so its `Debug` is what a test outside
/// `dom` can compare.
fn parse_of(svg: &[u8]) -> String {
    match ImageEvent::parse_document(Arc::from("app:///expected.svg"), svg, DocumentKind::Svg) {
        ImageEvent::ParsedDocument { document, .. } => format!("{document:?}"),
        other => panic!("the expected document does not parse: {other:?}"),
    }
}

/// A registered SVG document is not decoded: the fetcher parses it with the
/// engine's parser and reports the parsed document, keeping it (and no
/// bitmap) to answer later requests. It counts as its source's byte length.
/// Sizing is the engine's, pinned in `dom`.
#[test]
fn a_registered_svg_loads_as_the_document_it_parses_to() {
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="40" height="30px""#);
    harness
        .resources
        .register("app:///icon.svg", document.clone().into_bytes(), None)
        .expect("register");
    assert_eq!(
        format!("{:?}", harness.load_svg("app:///icon.svg")),
        parse_of(document.as_bytes())
    );
    assert_eq!(
        harness.resources.memory_used_bytes(),
        document.len(),
        "no bitmap; the parsed document, counted as its source's bytes"
    );
    assert!(harness.wakeups.load(Ordering::SeqCst) >= 1);
}

/// The fixtures inline SVGs as base64 `data:` URLs; one loads as the
/// document it carries.
#[test]
fn svg_data_urls_load_in_base64_form() {
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="16" height="8" viewBox="0 0 16 8""#);
    let source = format!(
        "data:image/svg+xml;base64,{}",
        base64_encode(document.as_bytes())
    );
    assert_eq!(
        format!("{:?}", harness.load_svg(&source)),
        parse_of(document.as_bytes()),
        "{source}"
    );
}

/// A document that does not parse is the load's failure, reported and
/// noted like a decode that failed, and terminal like one.
#[test]
fn a_malformed_svg_fails_its_source() {
    let harness = Harness::new(Harness::quiet());
    let broken = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect></svg>";
    harness
        .resources
        .register("app:///broken.svg", broken.to_vec(), None)
        .expect("register");
    let notes = harness.fail("app:///broken.svg");
    assert!(
        notes
            .iter()
            .any(|note| note.contains("not a document the engine can draw")),
        "{notes:?}"
    );
    assert!(harness.resources.knows_image("app:///broken.svg"));
    assert!(!harness.resources.is_resident("app:///broken.svg"));
    assert_eq!(harness.resources.memory_used_bytes(), 0);
    harness.fail("app:///broken.svg");
}

/// An SVG is the one image that is text, so it is recognised from its bytes
/// only where its label says nothing specific (`mime::sniff`): an
/// `application/octet-stream` label or no label at all loads, while a
/// `text/plain` label is trusted and is not an image.
#[test]
fn an_svg_is_sniffed_from_its_bytes_only_under_a_label_that_says_nothing() {
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="12" height="12""#);
    harness
        .resources
        .register(
            "app:///octet",
            document.clone().into_bytes(),
            Some("application/octet-stream"),
        )
        .expect("register");
    harness
        .resources
        .register("app:///unlabelled", document.clone().into_bytes(), None)
        .expect("register");
    harness
        .resources
        .register(
            "app:///text.svg",
            document.clone().into_bytes(),
            Some("text/plain"),
        )
        .expect("register");

    for source in ["app:///octet", "app:///unlabelled"] {
        assert_eq!(
            format!("{:?}", harness.load_svg(source)),
            parse_of(document.as_bytes()),
            "{source}"
        );
    }
    let notes = harness.fail("app:///text.svg");
    assert!(
        notes.iter().any(|note| note.contains("not an image")),
        "{notes:?}"
    );
}

/// A source already loaded answers a second request, from the same view or
/// another, at once and with the same document, shared rather than parsed
/// again.
#[test]
fn a_repeated_request_for_an_svg_re_reports_the_same_document() {
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="20" height="10""#);
    harness
        .resources
        .register("app:///repeat.svg", document.clone().into_bytes(), None)
        .expect("register");
    let first = harness.load_svg("app:///repeat.svg");

    harness.view.request_image("app:///repeat.svg");
    let (reports, inbox) = ImageInbox::new();
    let second = harness.resources.for_view(reports);
    second.request_image("app:///repeat.svg");
    for drained in [harness.inbox.drain(), inbox.drain()] {
        assert!(
            matches!(
                drained.as_slice(),
                [ImageEvent::ParsedDocument { source, document }]
                    if &**source == "app:///repeat.svg" && Arc::ptr_eq(document, &first)
            ),
            "{drained:?}"
        );
    }
}

/// Markup a page handed over arrives as a document request under the
/// synthetic source the engine named: nothing is resolved or fetched, the
/// bytes are parsed as a fetched document's are, and the entry answers a
/// repeated request, from any view, with the same document. Markup that does
/// not parse fails its source.
#[test]
fn a_document_request_is_parsed_under_its_synthetic_source() {
    const SOURCE: &str = "svg-content:0123456789abcdef0123456789abcdef";
    const BROKEN: &str = "svg-content:fedcba9876543210fedcba9876543210";
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="24" height="12""#);
    harness.view.request_document(
        SOURCE,
        bytes::Bytes::from(document.clone()),
        DocumentKind::Svg,
    );
    let first = harness.parsed(SOURCE);
    assert_eq!(format!("{first:?}"), parse_of(document.as_bytes()));
    assert!(harness.resources.knows_image(SOURCE));
    assert!(!harness.resources.is_resident(SOURCE));
    assert!(
        harness
            .view
            .read(SOURCE, ImageSizeHint::UNBOUNDED)
            .is_none()
    );
    assert_eq!(
        harness.resources.memory_used_bytes(),
        document.len(),
        "counted as the markup's bytes"
    );

    let (reports, inbox) = ImageInbox::new();
    let second = harness.resources.for_view(reports);
    second.request_document(SOURCE, bytes::Bytes::new(), DocumentKind::Svg);
    harness
        .view
        .request_document(SOURCE, bytes::Bytes::new(), DocumentKind::Svg);
    for drained in [harness.inbox.drain(), inbox.drain()] {
        assert!(
            matches!(
                drained.as_slice(),
                [ImageEvent::ParsedDocument { source, document }]
                    if &**source == SOURCE && Arc::ptr_eq(document, &first)
            ),
            "a repeat is answered from the entry: {drained:?}"
        );
    }

    harness.view.request_document(
        BROKEN,
        bytes::Bytes::from_static(b"<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>"),
        DocumentKind::Svg,
    );
    assert!(
        matches!(harness.settle(BROKEN), ImageEvent::Failed { .. }),
        "markup that does not parse fails"
    );
    assert!(
        harness
            .resources
            .take_notes()
            .iter()
            .all(|note| !note.contains("cannot be resolved")),
        "a synthetic source is never resolved"
    );
}

/// The engine draws a document itself, so the pixel seam has nothing for
/// it: no read answers, nothing is resident, and no read starts a
/// refinement. What the entry holds is the parsed document.
#[test]
fn an_svg_document_is_never_read_and_never_resident() {
    let harness = Harness::new(Harness::quiet());
    let document = svg(r#"width="64" height="64""#);
    harness
        .resources
        .register("app:///icon.svg", document.clone().into_bytes(), None)
        .expect("register");
    harness.load_svg("app:///icon.svg");

    assert!(harness.resources.knows_image("app:///icon.svg"));
    assert!(!harness.resources.is_resident("app:///icon.svg"));
    assert_eq!(harness.resources.resident_size("app:///icon.svg"), None);
    harness.view.retain(&[Arc::from("app:///icon.svg")]);
    for hint in [
        ImageSizeHint::UNBOUNDED,
        ImageSizeHint::new(8, 8),
        ImageSizeHint::new(4096, 4096),
    ] {
        assert!(
            harness.view.read("app:///icon.svg", hint).is_none(),
            "{hint:?}"
        );
    }
    std::thread::sleep(Duration::from_millis(50));
    harness.view.service_images();
    assert!(
        harness.inbox.drain().is_empty(),
        "nothing further is reported"
    );
    assert!(!harness.resources.is_resident("app:///icon.svg"));
    assert_eq!(harness.resources.memory_used_bytes(), document.len());
    assert!(harness.resources.take_notes().is_empty());
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut buffer = [0_u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from(buffer[0]) << 16 | u32::from(buffer[1]) << 8 | u32::from(buffer[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((bits >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
