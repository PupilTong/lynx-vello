//! The whole pipeline through the protocol's own seams, with no GPU and no
//! view: a `ViewResources` driven the way the painter drives it — request,
//! service, read — against an `ImageInbox` standing in for the document.
//!
//! Every image here goes through the real platform decoder, so these need
//! `ImageIO` or gdk-pixbuf and fail rather than skip without one.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceCapability, ResourceDescriptor,
    ResourceDescriptor as Descriptor, ResourceErrorKind, ResourceFetcher, ResourcePriority,
    ResourceRequest, ResourceSource, StyleSheetPayload,
};
use bobcat_core::{FrameImages, ImageEvent, ImageInbox, ImageSizeHint, PreparsedStyleSheet};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use http::HeaderMap;

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
            ImageEvent::Failed { .. } => {
                panic!("`{source}` failed: {:?}", self.resources.take_notes())
            }
        }
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

fn request_id(sequence: u64) -> RequestContext {
    RequestContext {
        id: RequestId {
            namespace: 7,
            sequence,
        },
        priority: ResourcePriority::Normal,
    }
}

async fn fetch(
    view: &ViewResources,
    specifier: &str,
) -> Result<bobcat_core::resource::ResourceResponse, bobcat_core::resource::ResourceError> {
    let resolved = view
        .resolve_locator(ResolveRequest {
            context: request_id(1),
            resource: ResourceDescriptor {
                specifier: Arc::from(specifier),
                base_url: None,
            },
            percent_decode: false,
        })
        .await?;
    view.fetch_resource(ResourceRequest {
        context: request_id(2),
        resource: resolved,
        headers: HeaderMap::new(),
        cache_policy: CachePolicy::Default,
    })
    .await
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
    harness.view.retain_images(&[Arc::from("app:///b.png")]);
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

#[tokio::test]
async fn text_resources_are_preprocessed_and_stylesheets_answer_pre_parsed() {
    let harness = Harness::new(Harness::quiet());
    harness
        .resources
        .register(
            "bobcat-memory://bundle/main.js",
            b"\xEF\xBB\xBFlet card = 1;".to_vec(),
            Some("text/javascript"),
        )
        .expect("register");
    let response = fetch(&harness.view, "bobcat-memory://bundle/main.js")
        .await
        .expect("fetch");
    assert_eq!(&response.bytes[..], b"let card = 1;", "the BOM is removed");
    assert_eq!(
        response.metadata.media_type.as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(response.metadata.source, ResourceSource::PackagedAsset);

    harness
        .resources
        .register_style_sheet(
            "bobcat-memory://bundle/style.css",
            PreparsedStyleSheet::default(),
        )
        .expect("register");
    assert!(
        harness
            .view
            .supports_capability(ResourceCapability::PreparsedStyleSheet)
    );
    let resolved = harness
        .view
        .resolve_locator(ResolveRequest {
            context: request_id(3),
            resource: Descriptor {
                specifier: Arc::from("bobcat-memory://bundle/style.css"),
                base_url: None,
            },
            percent_decode: false,
        })
        .await
        .expect("resolve");
    let sheet = harness
        .view
        .fetch_style_sheet(ResourceRequest {
            context: request_id(4),
            resource: resolved,
            headers: HeaderMap::new(),
            cache_policy: CachePolicy::Default,
        })
        .await
        .expect("style sheet");
    assert!(matches!(sheet.payload, StyleSheetPayload::Preparsed(_)));

    let missing = fetch(&harness.view, "app:///missing.js")
        .await
        .expect_err("unregistered");
    assert_eq!(missing.kind, ResourceErrorKind::UnsupportedScheme);

    let base = url::Url::parse("https://cards.test/app/").expect("a URL");
    harness.resources.set_base_url(Some(base));
    harness
        .resources
        .register(
            "https://cards.test/app/data.json",
            b"{\"ok\": true}".to_vec(),
            None,
        )
        .expect("register");
    let relative = fetch(&harness.view, "data.json")
        .await
        .expect("relative to the base");
    assert_eq!(
        relative.metadata.media_type.as_deref(),
        Some("application/json; charset=utf-8"),
        "labelled by extension and validated as JSON"
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
