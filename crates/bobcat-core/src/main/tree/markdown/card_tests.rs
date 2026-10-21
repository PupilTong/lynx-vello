//! The compiled `react-markdown` card
//! (`packages/reactlynx-test-fixtures/src/react-markdown/index.jsx`), end to
//! end over a real view: the public `rspeedy build` output's main-thread
//! script, its background thread loaded by bundle path, and the painter's
//! gesture path. What a card can only reach through the framework is pinned
//! here — a structured `markdown-style`, the events' background-thread
//! handlers, a selector-query `invoke` — and the shadow tree it builds is
//! read through a document probe.

use std::time::{Duration, Instant};

use terminus::link::Point2D;
use terminus::link::input::{InputEvent, PointerKind, PointerPhase};
use terminus::link::stylo::properties::PropertyId;

use super::super::LynxDocument;
use super::super::test_support::style_of;
use super::markdown_root;
use crate::test_support::{TestEngine, TestViewSpec, fixtures, wait_for_boot};
use crate::view::{EngineEvent, NoWakeup};

const AHEM: &[u8] = include_bytes!("../../../../../seldon/tests/fixtures/Ahem.ttf");

/// The URL the card's background thread registers its bundle at:
/// `requireModule('/app-service.js')` resolves under this container to
/// `app:///react-markdown.web.bundle/app-service.js`.
const BUNDLE_URL: &str = "app:///react-markdown.web.bundle";

/// The card's image, a 48 by 24 green PNG.
const IMAGE_PREFIX: &str = "data:image/png;base64,iVBORw0KGgo";

/// How long a wait for the background thread may take: a hang detector.
const PATIENCE: Duration = Duration::from_secs(30);

/// The card booted over a view whose default font is Ahem, as
/// `bobcat-source`'s `PageSource` would register it: the root module as the
/// entry, the background thread's boot script as `PageSource` writes it, and
/// every manifest body at the URL its bundle path names.
fn booted_card() -> TestEngine {
    let template = bobcat_source::web::decode(fixtures::fixture("react-markdown").page)
        .expect("the compiled card decodes");
    let background = format!(
        "import {{lynx, __BobcatRegisterBundle}} from 'bobcat:bts-runtime';\n\
         __BobcatRegisterBundle({url});\n\
         lynx.requireModule('/app-service.js');\n",
        url = serde_json::to_string(BUNDLE_URL).expect("a URL is a JavaScript string"),
    );
    let mut spec = TestViewSpec::new(&template.lepus_code["root"])
        .with_background_entry(&background)
        .with_default_font("Ahem", AHEM)
        .with_config(super::super::PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
            enable_js_data_processor: template.config_flag("enableJSDataProcessor"),
            enable_css_inheritance: template.config_flag("enableCSSInheritance"),
        });
    for (path, body) in &template.manifest {
        let source = if std::path::Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            body.clone()
        } else {
            bobcat_source::bts_module_source(bobcat_source::BundleTarget::Web, body)
        };
        spec = spec.with_module(&format!("{BUNDLE_URL}{path}"), &source);
    }
    let mut engine = spec.create(std::sync::Arc::new(NoWakeup));
    wait_for_boot(&mut engine);
    engine
}

/// Pumps the view until the card's background thread has logged every one
/// of `lines` (`markdown-card <name> <detail JSON>`), answering what it
/// logged on the way. A failure the view reports fails the wait.
fn await_logged(engine: &mut TestEngine, lines: &[&str]) -> Vec<String> {
    let deadline = Instant::now() + PATIENCE;
    let mut logged = Vec::new();
    loop {
        for event in engine.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. }
                    if message.starts_with("markdown-card ") =>
                {
                    logged.push(message);
                }
                failure @ (EngineEvent::ScriptRunError(_)
                | EngineEvent::ListenerFailed(_)
                | EngineEvent::WorkerThrew { .. }
                | EngineEvent::WorkerEnded { .. }) => panic!("the card failed: {failure:?}"),
                _ => {}
            }
        }
        if lines
            .iter()
            .all(|line| logged.iter().any(|seen| seen == line))
        {
            return logged;
        }
        assert!(
            Instant::now() < deadline,
            "expected {lines:?} from the background thread, saw {logged:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The element `selector` names in the light tree.
fn light(document: &LynxDocument, selector: &str) -> terminus::link::NodeId {
    document
        .query_selector(document.document_element().id(), selector)
        .expect("a valid selector")
        .unwrap_or_else(|| panic!("the card renders `{selector}`"))
}

/// Every element named `tag` in `host`'s shadow tree, in tree order.
fn shadow_all(
    document: &LynxDocument,
    host: terminus::link::NodeId,
    tag: &str,
) -> Vec<terminus::link::NodeId> {
    let mut found = Vec::new();
    let mut stack = vec![markdown_root(document, host).expect("the host's markdown root")];
    while let Some(id) = stack.pop() {
        let node = document.get(id).expect("a live shadow node");
        if node.tag_name() == Some(tag) {
            found.push(id);
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    found
}

/// The text nodes under `element`, concatenated.
fn text_of(document: &LynxDocument, element: terminus::link::NodeId) -> String {
    let mut text = String::new();
    let mut stack = vec![element];
    while let Some(id) = stack.pop() {
        let node = document.get(id).expect("a live node");
        if let Some(data) = node.text() {
            text.push_str(data);
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    text
}

/// One computed longhand of `element`, as a string.
fn computed(document: &LynxDocument, element: terminus::link::NodeId, property: &str) -> String {
    let id = PropertyId::parse_enabled_for_all_content(property).expect("a known property");
    let longhand = id.as_shorthand().expect_err("a longhand");
    style_of(document, element).computed_value_to_string(longhand)
}

/// The card's `<x-markdown>`, compiled: the structured `markdown-style`
/// reaches the element as its JSON text, the shadow tree holds web-core's
/// `basic` fixture with the link and the image after it, the link is the
/// `markdown-style` red and the inline code its blue, and the sibling
/// `<markdown>` renders under native's tag.
///
/// On the background thread: the render's `parseEnd` reaches `bindparseEnd`
/// with `{id: "case-1"}`, and the mount effect's selector-query `invoke` of
/// `getContent({start: 0, end: 6})` answers `{content: "# Title"}`. Then a
/// tap on the link's text, through the painter's gesture path, reaches
/// `bindlink` with web-core's `{url, content, contentId}`.
#[test]
fn a_compiled_card_renders_its_markdown_and_hears_its_events() {
    let mut engine = booted_card();
    await_logged(
        &mut engine,
        &[
            r#"markdown-card parseEnd {"id":"case-1"}"#,
            r##"markdown-card getContent {"content":"# Title"}"##,
        ],
    );

    let tap_at = engine
        .probe_document(|document| {
            document.layout();
            let host = light(document, "#md");
            let node = document.get(host).expect("the host");
            assert_eq!(node.tag_name(), Some("x-markdown"));
            assert_eq!(
                node.attribute("markdown-style").as_deref(),
                Some(r#"{"link":{"color":"ff0000"},"inlineCode":{"color":"0000ff"}}"#),
                "an object-valued markdown-style is written as its JSON text"
            );
            assert_eq!(node.attribute("content-id").as_deref(), Some("case-1"));

            let headings = shadow_all(document, host, "h1");
            assert_eq!(headings.len(), 1);
            assert_eq!(text_of(document, headings[0]), "Title");
            let links = shadow_all(document, host, "a");
            assert_eq!(links.len(), 1);
            let link = links[0];
            assert_eq!(
                document
                    .get(link)
                    .and_then(|node| node.attribute("href"))
                    .as_deref(),
                Some("https://lynxjs.org/")
            );
            assert_eq!(text_of(document, link), "link");
            assert_eq!(computed(document, link, "color"), "rgb(255, 0, 0)");
            let code = shadow_all(document, host, "code");
            assert_eq!(text_of(document, code[0]), "code");
            assert_eq!(computed(document, code[0], "color"), "rgb(0, 0, 255)");
            let images = shadow_all(document, host, "image");
            assert_eq!(images.len(), 1);
            assert!(
                document
                    .get(images[0])
                    .and_then(|node| node.attribute("src"))
                    .is_some_and(|src| src.starts_with(IMAGE_PREFIX))
            );

            let plain = light(document, "#plain");
            assert_eq!(
                document.get(plain).and_then(terminus::link::Node::tag_name),
                Some("markdown")
            );
            let strong = shadow_all(document, plain, "strong");
            assert_eq!(strong.len(), 1);
            assert_eq!(text_of(document, strong[0]), "plain");

            // The middle of the link's text, in viewport px: its paragraph's
            // content origin plus the centre of the box its source range
            // occupies.
            let paragraph = document.paragraph_of(link).expect("the link's paragraph");
            let range = document
                .inline_source_range(link)
                .expect("the link's source range");
            let boxes = document
                .text_range_boxes(
                    paragraph,
                    u32::try_from(range.start).unwrap(),
                    u32::try_from(range.end).unwrap(),
                )
                .expect("the link's boxes");
            let origin =
                super::methods::content_origin(document, paragraph).expect("a laid-out paragraph");
            boxes[0].center() + origin
        })
        .expect("the probe runs");

    for phase in [PointerPhase::Down, PointerPhase::Up] {
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(tap_at.x, tap_at.y),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    await_logged(
        &mut engine,
        &[
            r#"markdown-card link {"url":"https://lynxjs.org/","content":"link","contentId":"case-1"}"#,
        ],
    );
}
