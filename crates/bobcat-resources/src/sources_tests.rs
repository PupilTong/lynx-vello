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

/// A container installer that writes one script beside the URL it was given,
/// out of the bytes it was handed, and records the URL it was based on.
///
/// It stands in for the three answers the hook has: bytes it does not
/// recognize are `Ok(false)` and change nothing, bytes it cannot decode are
/// an error that fails the fetch, and anything else is installed.
#[derive(Debug)]
struct ScriptInstaller {
    installed: Mutex<Vec<Url>>,
}

impl crate::ContainerInstaller for ScriptInstaller {
    fn install(
        &self,
        url: &Url,
        bytes: &[u8],
        registrar: &crate::Registrar,
    ) -> Result<bool, String> {
        self.installed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(url.clone());
        if bytes == b"not a container" {
            return Err("these bytes are not a container".to_owned());
        }
        if bytes == b"an ordinary file" {
            return Ok(false);
        }
        registrar
            .register(
                &format!("{url}/x.js"),
                bytes.to_vec(),
                Some("text/javascript"),
            )
            .map_err(|error| error.to_string())?;
        Ok(true)
    }
}

fn installing(installer: Arc<ScriptInstaller>) -> Resources {
    Resources::new(
        ResourcesConfig {
            base_url: Some(Url::parse("app:///page/main.js").unwrap()),
            log_to_stderr: false,
            container_installer: Some(installer),
            ..ResourcesConfig::default()
        },
        || panic!("container loads do not wake the image host"),
    )
}

/// A host that installs nothing still *fetches*: the request is a plain
/// fetch, as an image's is, and having nothing to make of the bytes is not a
/// failure.
#[tokio::test]
async fn a_fetch_completes_where_the_host_installs_no_containers() {
    let resources = resources();
    resources
        .register("app:///page/lazy.bundle", b"payload".to_vec(), None)
        .unwrap();

    assert!(matches!(
        result(load(
            &resources,
            "app:///page/lazy.bundle",
            SourceKind::Fetch
        ))
        .await,
        Ok(LoadedSource::Fetched)
    ));
}

/// An installer that does not recognize the bytes leaves the fetch alone:
/// completed, with nothing registered.
#[tokio::test]
async fn an_unrecognized_body_completes_the_fetch_and_registers_nothing() {
    let installer = Arc::new(ScriptInstaller {
        installed: Mutex::new(Vec::new()),
    });
    let resources = installing(Arc::clone(&installer));
    resources
        .register("app:///page/photo.png", b"an ordinary file".to_vec(), None)
        .unwrap();

    assert!(matches!(
        result(load(&resources, "app:///page/photo.png", SourceKind::Fetch)).await,
        Ok(LoadedSource::Fetched)
    ));
    // Offered the bytes, and wrote nothing: the section URL a container would
    // have answered at is still nothing.
    assert_eq!(installer.installed.lock().unwrap().len(), 1);
    assert!(
        result(load(
            &resources,
            "app:///page/photo.png/x.js",
            SourceKind::Script
        ))
        .await
        .is_err()
    );
}

/// The whole point of installing: the URLs the container's sections answer at
/// become ordinary source requests afterwards.
#[tokio::test]
async fn an_installed_container_answers_the_module_requests_it_registered() {
    let installer = Arc::new(ScriptInstaller {
        installed: Mutex::new(Vec::new()),
    });
    let resources = installing(Arc::clone(&installer));
    resources
        .register(
            "app:///lazy-bundle/child.bundle",
            b"export default 1;".to_vec(),
            None,
        )
        .unwrap();

    assert!(matches!(
        result(load(
            &resources,
            "app:///lazy-bundle/child.bundle",
            SourceKind::Fetch
        ))
        .await,
        Ok(LoadedSource::Fetched)
    ));

    let Ok(LoadedSource::Module { source, url }) = result(load(
        &resources,
        "app:///lazy-bundle/child.bundle/x.js",
        SourceKind::Script,
    ))
    .await
    else {
        panic!("the section the installer registered answers a module request");
    };
    assert_eq!(source, "export default 1;");
    assert_eq!(url, "app:///lazy-bundle/child.bundle/x.js");
    // The resolved *request* URL, which is what the realm derives its section
    // URLs from — a rooted specifier resolving at the page's origin root.
    assert_eq!(
        installer
            .installed
            .lock()
            .unwrap()
            .iter()
            .map(Url::to_string)
            .collect::<Vec<_>>(),
        ["app:///lazy-bundle/child.bundle"]
    );
}

/// A rooted specifier is what a compiled card writes (webpack's `publicPath`
/// is `/`), and what the installer is based on is where that resolved to —
/// the page's origin root, not the directory the page sits in.
///
/// `request` maps a `Fetch` through the scope's own base URL, exactly as it
/// maps an entry or a font, so what that mapping produces is this
/// resolution.
#[test]
fn a_rooted_container_specifier_resolves_at_the_pages_origin_root() {
    let resources = resources();
    // Registered so a scheme no transport serves still resolves, which is
    // how a container reaches a test at all.
    resources
        .register("app:///lazy-bundle/child.bundle", b"body".to_vec(), None)
        .unwrap();

    let resolved = resources
        .shared
        .transports
        .resolve("/lazy-bundle/child.bundle", resources.base_url().as_ref())
        .expect("a rooted specifier resolves against the page");

    assert_eq!(resolved.as_str(), "app:///lazy-bundle/child.bundle");
}

/// An installer that cannot decode what it recognized fails the fetch, and
/// its message is what the realm reports.
#[tokio::test]
async fn an_installer_that_refuses_the_bytes_reports_its_own_message() {
    let installer = Arc::new(ScriptInstaller {
        installed: Mutex::new(Vec::new()),
    });
    let resources = installing(installer);
    resources
        .register("app:///page/bad.bundle", b"not a container".to_vec(), None)
        .unwrap();

    let Err(LynxViewError::Resource(error)) = result(load(
        &resources,
        "app:///page/bad.bundle",
        SourceKind::Fetch,
    ))
    .await
    else {
        panic!("the installer's refusal fails the request");
    };

    assert_eq!(error.kind, ResourceErrorKind::ResponseBody);
    assert!(error.message.contains("not a container"));
}

/// The probe's whole contract: a plain fetch that **completed** is `true`
/// afterwards, by the specifier the realm holds as well as by its resolved
/// form, and nothing else is.
#[tokio::test]
async fn a_completed_fetch_is_what_the_probe_answers_true_for() {
    let installer = Arc::new(ScriptInstaller {
        installed: Mutex::new(Vec::new()),
    });
    let resources = installing(Arc::clone(&installer));
    resources
        .register(
            "app:///lazy-bundle/child.bundle",
            b"export default 1;".to_vec(),
            None,
        )
        .unwrap();
    let (reports, _inbox) = bobcat_core::ImageInbox::new();
    let probe = bobcat_core::resource::ResourceFetcher::fetch_probe(&resources.for_view(reports))
        .expect("the reference fetcher always offers a probe");

    // Nothing has been fetched yet, whatever else is registered.
    assert!(!probe("app:///lazy-bundle/child.bundle"));

    assert!(matches!(
        result(load(
            &resources,
            "app:///lazy-bundle/child.bundle",
            SourceKind::Fetch
        ))
        .await,
        Ok(LoadedSource::Fetched)
    ));

    // The URL the realm passed, and the rooted spelling of it a compiled card
    // writes: both resolve to the one URL the fetch completed for.
    assert!(probe("app:///lazy-bundle/child.bundle"));
    assert!(probe("/lazy-bundle/child.bundle"));
    // A URL nothing fetched, and one that resolves to nothing at all.
    assert!(!probe("app:///lazy-bundle/other.bundle"));
    assert!(!probe("::not a url::"));
}

/// A fetch that failed is not remembered, as native remembers no failure, and
/// a load of another kind never populates the set at all.
#[tokio::test]
async fn a_failed_fetch_and_every_other_kind_leave_the_probe_false() {
    let resources = resources();
    resources
        .register("app:///page/sheet.css", b".a {}".to_vec(), Some("text/css"))
        .unwrap();
    resources
        .register(
            "app:///page/module.js",
            b"export default 1;".to_vec(),
            Some("text/javascript"),
        )
        .unwrap();
    let (reports, _inbox) = bobcat_core::ImageInbox::new();
    let probe = bobcat_core::resource::ResourceFetcher::fetch_probe(&resources.for_view(reports))
        .expect("the reference fetcher always offers a probe");

    // Nothing is registered under this URL, so the fetch fails.
    assert!(
        result(load(
            &resources,
            "app:///page/gone.bundle",
            SourceKind::Fetch
        ))
        .await
        .is_err()
    );
    assert!(!probe("app:///page/gone.bundle"));

    // A stylesheet and a module complete, and neither is a fetch.
    assert!(
        result(load(
            &resources,
            "app:///page/sheet.css",
            SourceKind::StyleSheet
        ))
        .await
        .is_ok()
    );
    assert!(
        result(load(
            &resources,
            "app:///page/module.js",
            SourceKind::Script
        ))
        .await
        .is_ok()
    );
    assert!(!probe("app:///page/sheet.css"));
    assert!(!probe("app:///page/module.js"));
}
