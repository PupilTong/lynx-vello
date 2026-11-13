//! `.lynx.bundle`s whose scripts are `PrimJS` bytecode, as containers: the
//! URL and kind each converted section is registered under, the module
//! name written into it, refusals, and the four hand-written bytecode
//! cards booted against their source twins.
// The cards boot views on native threads.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::native::{ConvertError, primjs};
use bobcat_source::{BundleInstaller, ContainerSources, PageSource, SourceError};
use quickjs_rust_bridge::bytecode as fork;
use url::Url;

/// The screen these tests' views report.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(200.0, 200.0, 1.0);

fn sample(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/primjs/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The module name written into converted bytes.
fn module_name(bytes: &[u8]) -> String {
    let program = fork::read_program(bytes).expect("converted bytes read");
    let fork::Value::Module(module) = &program.root else {
        panic!("a converted section is a module");
    };
    program
        .atom_text(module.name)
        .expect("a named module")
        .into_owned()
}

/// `sections.lynx.bundle` carries `ROOT_LEPUS`, one `LEPUS_CHUNK` entry, one
/// `JS_BYTECODE` file and one `JsBytecode` custom section. Each is
/// converted and registered as module bytecode under the URL the section
/// mapping gives it, with that URL as its module name; the web-bundle
/// conversion still refuses the bundle.
#[test]
fn every_bytecode_section_is_registered_at_its_url() {
    let bytes = sample("sections.lynx.bundle");
    let sections: Vec<(primjs::SectionKind, String)> = primjs::bundle_bytecode_sections(&bytes)
        .unwrap()
        .into_iter()
        .map(|section| (section.kind, section.name))
        .collect();
    eprintln!("sections: {sections:?}");
    let url = Url::parse("https://cdn.test/app/sections.lynx.bundle?v=1").unwrap();
    let container = ContainerSources::from_bytes(&url, &bytes).unwrap();
    let registered: Vec<(&str, String)> = container
        .module_bytecode()
        .iter()
        .map(|(at, bytes)| (at.as_str(), module_name(bytes)))
        .collect();
    let mut expected = BTreeSet::new();
    for (kind, name) in &sections {
        let bare = name.strip_prefix('/').unwrap_or(name);
        let at = match kind {
            primjs::SectionKind::RootLepus => "root.js".to_owned(),
            primjs::SectionKind::LepusChunk | primjs::SectionKind::CustomSection => {
                format!("{bare}.js")
            }
            primjs::SectionKind::JsBytecode => bare.to_owned(),
        };
        expected.insert(format!(
            "https://cdn.test/app/sections.lynx.bundle/{at}?v=1"
        ));
    }
    let actual: BTreeSet<String> = registered.iter().map(|(at, _)| (*at).to_owned()).collect();
    assert_eq!(actual, expected);
    for (at, name) in &registered {
        assert_eq!(at, name, "the module is named by its URL");
    }
    assert_eq!(
        container.entry().map(Url::as_str),
        Some("https://cdn.test/app/sections.lynx.bundle/root.js?v=1")
    );
    // No text body at any bytecode URL.
    for (at, _) in container.scripts() {
        assert!(!actual.contains(at.as_str()), "{at} is text and bytecode");
    }
    for result in [
        bobcat_source::native::decode(&bytes).map(drop),
        bobcat_source::native::convert(&bytes).map(drop),
    ] {
        assert!(matches!(result, Err(ConvertError::CodeCacheBundle { .. })));
    }
}

/// The names written into a real fixture's converted sections are the
/// URLs the realm requests: the entry the view imports, the background
/// script the BTS boot requires.
#[test]
fn module_names_are_the_urls_the_realm_requests() {
    let fixture = bytecode::fixture("react-native");
    let input = Url::parse("app:///react-native.lynx.bundle").unwrap();
    let page = PageSource::from_bytes(&input, fixture.js_bytecode_page).unwrap();
    let sources = page.view_sources(SCREEN, "en-US");
    let container = ContainerSources::from_bytes(&input, fixture.js_bytecode_page).unwrap();
    let entry = container
        .module_bytecode()
        .iter()
        .find(|(at, _)| at.as_str() == sources.entry)
        .expect("the entry the view imports is module bytecode");
    assert_eq!(module_name(&entry.1), sources.entry);
    let app_service = container
        .module_bytecode()
        .iter()
        .find(|(at, _)| at.path().ends_with("/app-service.js"))
        .expect("`/app-service.js` is module bytecode");
    assert_eq!(module_name(&app_service.1), app_service.0.as_str());
    // `requireModule('/app-service.js')` resolves against the container URL.
    assert_eq!(
        app_service.0,
        input
            .join("react-native.lynx.bundle/app-service.js")
            .unwrap()
    );
}

/// A refused section refuses the whole container, naming the section and
/// the reason.
#[test]
fn a_refused_section_refuses_the_container() {
    let card = bytecode::card("refused-lazy");
    let (path, bytes) = card.chunks[0];
    let url = Url::parse(&format!("https://cdn.test/{path}")).unwrap();
    let error = ContainerSources::from_bytes(&url, bytes).unwrap_err();
    let SourceError::DecodeNativeBundle {
        source:
            ConvertError::Bytecode {
                section,
                kind,
                reason,
            },
        ..
    } = &error
    else {
        panic!("not a bytecode refusal: {error}");
    };
    assert_eq!(section, "main-thread");
    assert_eq!(*kind, primjs::SectionKind::CustomSection);
    assert!(reason.contains("eval"), "{reason}");
    let message = error.to_string();
    assert!(
        message.contains("main-thread") && message.contains("eval"),
        "{message}"
    );
}

/// A deferred native page is decoded, and so converted, when it is
/// deferred: a refusal is the defer's error, as for `from_bytes`.
#[test]
fn a_deferred_bytecode_page_converts_when_deferred() {
    let input = Url::parse("app:///page.lynx.bundle").unwrap();
    let fixture = bytecode::fixture("react-native");
    let deferred = PageSource::defer(&input, fixture.page).unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    deferred.register_in_background(&resources);
    let development = bytecode::fixture("react-reload-development");
    let error = PageSource::defer(&input, development.page).unwrap_err();
    assert!(error.to_string().contains("ROOT_LEPUS"), "{error}");
}

/// The four hand-written cards, each bytecode form against its source
/// twin: the console lines the README lists.
#[tokio::test]
async fn cards_run_from_bytecode_as_their_source_twins_do() {
    for card in bytecode::CARDS {
        for (form, page) in [("page", card.page), ("js-bytecode", card.js_bytecode_page)] {
            let label = format!("{} {form}", card.name);
            let input =
                Url::parse(&format!("https://cdn.test/cards/{}.lynx.bundle", card.name)).unwrap();
            let page = PageSource::from_bytes(&input, page).unwrap();
            let lines = run(&label, &input, &page, card.chunks, card.expected).await;
            check_card(card.name, &label, &lines, card.expected);
        }
        let label = format!("{} source", card.name);
        let input =
            Url::parse(&format!("https://cdn.test/cards/{}.web.bundle", card.name)).unwrap();
        let source = &card.source;
        let page = if source.lepus_chunks.is_empty() {
            PageSource::from_native_bundle(&input, source.page, source.entry).unwrap()
        } else {
            let mut renames = vec![(source.entry, "root")];
            renames.extend(source.lepus_chunks.iter().copied());
            PageSource::from_bytes(&input, &web_page_renaming(source.page, &renames)).unwrap()
        };
        let lines = run(&label, &input, &page, source.chunks, source.expected).await;
        check_card(card.name, &label, &lines, source.expected);
    }
}

fn check_card(name: &str, label: &str, lines: &[String], expected: &[&str]) {
    for line in expected {
        assert!(
            lines.iter().any(|seen| seen == line),
            "{label}: {line:?} not in {lines:?}"
        );
    }
    if name == "refused-lazy" && label.ends_with("source") {
        return;
    }
    if name == "refused-lazy" {
        // The fetch's reason names the refused section and why.
        let reason = lines
            .iter()
            .find(|line| line.starts_with("refused-lazy error_msg "))
            .unwrap_or_else(|| panic!("{label}: no error_msg in {lines:?}"));
        assert!(
            reason.contains("main-thread") && reason.contains("eval"),
            "{reason}"
        );
    }
}

/// Boots `page` with its lazy `chunks` beside it and a container installer,
/// and collects console lines until every `expected` one was logged. Any
/// error report fails the run.
async fn run(
    label: &str,
    input: &Url,
    page: &PageSource,
    chunks: &[(&str, &'static [u8])],
    expected: &[&str],
) -> Vec<String> {
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            container_installer: Some(Arc::new(BundleInstaller)),
            ..ResourcesConfig::default()
        },
        || {},
    );
    page.register_with(&resources);
    for (path, bytes) in chunks {
        resources
            .register(input.join(path).unwrap().as_str(), *bytes, None)
            .unwrap();
    }
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(
            200.0,
            200.0,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter =
        bobcat_core::Painter::new(bobcat_core::DrawTarget::Offscreen, 200.0, 200.0, 1.0)
            .await
            .unwrap();
    painter.attach(&view).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut lines = Vec::new();
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {}
                EngineEvent::ConsoleMessage { message, .. } => {
                    eprintln!("{label}: {message}");
                    lines.push(message);
                }
                other => panic!("{label}: unexpected runtime event: {other:?}"),
            }
        }
        painter.pump().unwrap();
        if expected
            .iter()
            .all(|line| lines.iter().any(|seen| seen == line))
        {
            // A little longer, for lines logged after the expected ones.
            let settle = Instant::now() + Duration::from_millis(50);
            while Instant::now() < settle {
                for event in view.pump() {
                    if let EngineEvent::ConsoleMessage { message, .. } = event {
                        lines.push(message);
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            return lines;
        }
        assert!(Instant::now() < deadline, "{label}: only {lines:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `native` converted to a web bundle, with each `(from, to)` Lepus chunk
/// renamed, as `tests/conversion.rs` does: a `.lynx.bundle` cannot carry a
/// source Lepus chunk under its own name.
fn web_page_renaming(native: &[u8], renames: &[(&str, &str)]) -> Vec<u8> {
    fn string(output: &mut Vec<u8>, value: &str) {
        output.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
        output.extend_from_slice(value.as_bytes());
    }
    let mut bytes = bobcat_source::native::convert(native).unwrap();
    let mut scripts = bobcat_source::web::decode(&bytes).unwrap().lepus_code;
    for (from, to) in renames {
        let source = scripts.remove(*from).unwrap();
        scripts.insert((*to).to_owned(), source);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(&u32::try_from(scripts.len()).unwrap().to_le_bytes());
    for (name, source) in scripts {
        string(&mut payload, &name);
        string(&mut payload, &source);
    }
    let mut offset = 12;
    loop {
        let tag = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if tag == bobcat_source::web::SectionLabel::LepusCode as u32 {
            bytes[offset + 4..offset + 8]
                .copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            bytes.splice(offset + 8..offset + 8 + len, payload);
            return bytes;
        }
        offset += 8 + len;
    }
}
