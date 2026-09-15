use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use super::*;
use crate::ResourcesConfig;

fn resources() -> Resources {
    Resources::new(
        ResourcesConfig {
            base_url: Some(Url::parse("app:///page/main.js").unwrap()),
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || panic!("stylesheet loads do not wake the image host"),
    )
}

async fn result(mut response: CachedStyle) -> SourceResult {
    tokio::time::timeout(Duration::from_secs(10), response.wait_for(Option::is_some))
        .await
        .expect("stylesheet load finishes")
        .expect("loader answers")
        .as_ref()
        .unwrap()
        .clone()
}

async fn text(response: CachedStyle) -> String {
    match result(response).await.unwrap() {
        LoadedSource::StyleSheet(StyleSheetSource::Text(text)) => text,
        source => panic!("expected CSS text, got {source:?}"),
    }
}

/// One load of any kind, over the same watch channel the style cache uses —
/// `SourceCompletion`'s two ends are minted inside `bobcat-core`, and a
/// destination is all this needs.
fn load(resources: &Resources, url: &str, kind: SourceKind) -> CachedStyle {
    let (sender, response) = watch::channel(None);
    start(
        resources,
        Url::parse(url).expect("a URL"),
        kind,
        Destination::Cache(sender),
    );
    response
}

async fn font(resources: &Resources, url: &str) -> Vec<u8> {
    match result(load(resources, url, SourceKind::Font))
        .await
        .unwrap()
    {
        LoadedSource::Font(blob) => blob.as_ref().to_vec(),
        source => panic!("expected font bytes, got {source:?}"),
    }
}

/// The vendored Ahem face, the one font fixture this workspace ships.
const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

#[tokio::test]
async fn a_font_source_is_served_from_disk_without_utf8_validation() {
    let resources = resources();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../hughie/tests/fixtures/Ahem.ttf"
    );

    let bytes = font(&resources, &format!("file://{path}")).await;

    assert_eq!(bytes, AHEM, "the face arrives byte for byte");
    // The point of the assertion: a `.ttf` is not UTF-8, so a stylesheet or a
    // script load of the same URL would have been refused for its encoding.
    assert!(std::str::from_utf8(&bytes).is_err());
}

#[tokio::test]
async fn a_font_source_is_served_from_a_data_url() {
    let resources = resources();

    let bytes = font(&resources, "data:font/ttf;base64,AAEAAAA=").await;

    assert_eq!(bytes, [0x00, 0x01, 0x00, 0x00, 0x00]);
}

#[tokio::test]
async fn normalized_preloads_and_reads_share_pending_and_completed_work() {
    let resources = resources();
    let url = resources
        .register(
            "app:///page/a.css",
            ".box { width: 40px; }",
            Some("text/css"),
        )
        .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let (started, pending) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    resources.shared.set_fetch_hook(Arc::new({
        let reads = Arc::clone(&reads);
        move |_| {
            reads.fetch_add(1, Ordering::SeqCst);
            started.send(()).unwrap();
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }));
    preload(&resources, SourceRequest::StyleSheet("./a.css".into()));
    pending.recv_timeout(Duration::from_secs(10)).unwrap();
    preload(
        &resources,
        SourceRequest::StyleSheet("./sub/../a.css".into()),
    );
    let first = cached_style(&resources, url.clone());
    let second = cached_style(&resources.clone(), url.clone());
    assert!(first.borrow().is_none());
    release.send(()).unwrap();
    assert_eq!(text(first).await, ".box { width: 40px; }");
    assert_eq!(text(second).await, ".box { width: 40px; }");
    assert_eq!(
        text(cached_style(&resources, url)).await,
        ".box { width: 40px; }"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cached_failures_and_registration_changes_keep_their_source_semantics() {
    let resources = resources();
    let url = Url::parse("app:///page/a.css").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    resources.shared.set_fetch_hook(Arc::new({
        let reads = Arc::clone(&reads);
        move |_| {
            reads.fetch_add(1, Ordering::SeqCst);
        }
    }));
    for _ in 0..2 {
        let Err(LynxViewError::Resource(error)) =
            result(cached_style(&resources, url.clone())).await
        else {
            panic!("unregistered app URL must preserve its resource error");
        };
        assert_eq!(error.kind, ResourceErrorKind::UnsupportedScheme);
    }
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    for css in [".box { width: 40px; }", ".box { width: 80px; }"] {
        resources
            .register(url.as_str(), css, Some("text/css"))
            .unwrap();
        assert_eq!(text(cached_style(&resources, url.clone())).await, css);
    }
    assert!(resources.unregister(url.as_str()));
    assert!(result(cached_style(&resources, url.clone())).await.is_err());
    resources
        .register(url.as_str(), ".box {}", Some("text/css"))
        .unwrap();
    assert_eq!(text(cached_style(&resources, url.clone())).await, ".box {}");
    resources.clear_registered();
    assert!(result(cached_style(&resources, url.clone())).await.is_err());
    resources
        .register_style_sheet(url.as_str(), bobcat_core::PreparsedStyleSheet::default())
        .unwrap();
    preload(&resources, SourceRequest::StyleSheet(url.to_string()));
    assert!(
        !resources.style_cache.borrow().contains_key(&url),
        "registered preparsed CSS needs no cache load"
    );
}

#[tokio::test]
async fn replacement_scope_and_registration_ignore_retired_load_results() {
    let resources = resources();
    let url = resources
        .register("app:///page/a.css", ".old {}", Some("text/css"))
        .unwrap();
    let retired = cached_style(&resources, url.clone());
    assert_eq!(text(retired.clone()).await, ".old {}");
    resources
        .register(url.as_str(), ".new {}", Some("text/css"))
        .unwrap();
    assert_eq!(text(cached_style(&resources, url.clone())).await, ".new {}");
    // A consumer can still own the old result; it cannot replace the new cache entry.
    assert_eq!(text(retired).await, ".old {}");
    assert_eq!(text(cached_style(&resources, url.clone())).await, ".new {}");
    let replacement = resources.new_scope();
    assert!(
        result(cached_style(&replacement, url.clone()))
            .await
            .is_err()
    );
    replacement
        .register(url.as_str(), ".replacement {}", Some("text/css"))
        .unwrap();
    assert_eq!(
        text(cached_style(&replacement, url.clone())).await,
        ".replacement {}"
    );
    assert_eq!(text(cached_style(&resources, url)).await, ".new {}");
}
