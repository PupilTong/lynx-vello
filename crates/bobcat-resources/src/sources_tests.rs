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
const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

#[tokio::test]
async fn a_font_source_is_served_from_disk_without_utf8_validation() {
    let resources = resources();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../seldon/tests/fixtures/Ahem.ttf"
    );

    let bytes = font(&resources, &format!("file://{path}")).await;

    assert_eq!(bytes, AHEM, "the face arrives byte for byte");
    // The point of the assertion: a `.ttf` is not UTF-8, so a stylesheet or a
    // script load of the same URL would have been refused for its encoding.
    assert!(std::str::from_utf8(&bytes).is_err());
}

#[tokio::test]
async fn every_view_loading_one_font_file_is_handed_the_same_mapping() {
    let resources = resources();
    let url = format!(
        "file://{}",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../seldon/tests/fixtures/Ahem.ttf"
        )
    );
    let blob = async |resources: &Resources| match result(load(resources, &url, SourceKind::Font))
        .await
        .unwrap()
    {
        LoadedSource::Font(blob) => blob,
        source => panic!("expected font bytes, got {source:?}"),
    };

    // Two views' scopes, as two `LynxView`s on one system hold.
    let first = blob(&resources).await;
    let second = blob(&resources.new_scope()).await;

    assert_eq!(first.as_ref(), AHEM);
    // Not a heap copy each: the file's one mapping, which `FontBlob::new`
    // answers with one blob.
    assert_eq!(first.as_ref().as_ptr(), second.as_ref().as_ptr());
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
        .register_style_sheet(
            url.as_str(),
            Arc::new(bobcat_core::PreparsedStyleSheet::default()),
        )
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

impl crate::ContainerInstaller<bobcat_core::PreparsedStyleSheet> for ScriptInstaller {
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
            SourceKind::Fetch { timed: false }
        ))
        .await,
        Ok(LoadedSource::Fetched(_))
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
        result(load(
            &resources,
            "app:///page/photo.png",
            SourceKind::Fetch { timed: false }
        ))
        .await,
        Ok(LoadedSource::Fetched(_))
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
            SourceKind::Fetch { timed: false }
        ))
        .await,
        Ok(LoadedSource::Fetched(_))
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
        SourceKind::Fetch { timed: false },
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
            SourceKind::Fetch { timed: false }
        ))
        .await,
        Ok(LoadedSource::Fetched(_))
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
            SourceKind::Fetch { timed: false }
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

/// The stylesheet probe reads the registry and nothing else: a registered
/// sheet — pre-parsed or CSS bytes — is `true` by any specifier that resolves
/// to it, its removal makes it `false` again, and a URL nothing registered is
/// `false` without any load being started for it.
#[test]
fn the_style_sheet_probe_answers_from_the_registry_alone() {
    let resources = resources();
    resources
        .register_style_sheet(
            "app:///page/child.bundle/custom_section/CSS.css",
            Arc::new(bobcat_core::PreparsedStyleSheet { rules: Vec::new() }),
        )
        .unwrap();
    resources
        .register("app:///page/text.css", b".a {}".to_vec(), Some("text/css"))
        .unwrap();
    let (reports, _inbox) = bobcat_core::ImageInbox::new();
    let probe =
        bobcat_core::resource::ResourceFetcher::style_sheet_probe(&resources.for_view(reports))
            .expect("the reference fetcher always offers a probe");

    assert!(probe("app:///page/child.bundle/custom_section/CSS.css"));
    // Resolved against the base the way a request is.
    assert!(probe("child.bundle/custom_section/CSS.css"));
    assert!(probe("/page/text.css"));
    // Nothing registered here: `false`, and over HTTP too, where a request
    // would have gone to the network.
    assert!(!probe("app:///page/other.bundle/custom_section/CSS.css"));
    assert!(!probe(
        "https://cdn.test/child.bundle/custom_section/CSS.css"
    ));
    assert!(!probe("::not a url::"));
    assert!(resources.unregister("app:///page/text.css"));
    assert!(!probe("app:///page/text.css"));
}

/// A background install whose body waits for the test to let it go, so a
/// request can be shown waiting on it.
fn install_held(
    resources: &Resources,
    container: &str,
    outcome: Result<(), String>,
) -> mpsc::Sender<()> {
    let (release, released) = mpsc::channel::<()>();
    let container = Url::parse(container).unwrap();
    let section = format!("{container}/root.js");
    let sheet = format!("{container}/style-info.css");
    let bytecode = format!("{container}/chunk.js");
    resources.register_in_background(&container, move |registrar| {
        released.recv().expect("the test lets the install go");
        registrar
            .register(
                &section,
                b"export default 2;".to_vec(),
                Some("text/javascript"),
            )
            .unwrap();
        registrar
            .register_style_sheet(
                &sheet,
                Arc::new(bobcat_core::PreparsedStyleSheet::default()),
            )
            .unwrap();
        registrar
            .register_module_bytecode(&bytecode, b"module bytecode".to_vec())
            .unwrap();
        outcome
    });
    release
}

/// A view boots while its container is still being registered: a load of a
/// URL under the container waits for the install, then finds what it
/// registered — a module as bytes, a stylesheet pre-parsed.
#[tokio::test]
async fn a_load_under_a_container_being_registered_waits_for_it() {
    let resources = resources();
    let release = install_held(&resources, "app:///page/card.web.bundle", Ok(()));

    let mut module = load(
        &resources,
        "app:///page/card.web.bundle/root.js",
        SourceKind::Script,
    );
    let sheet = load(
        &resources,
        "app:///page/card.web.bundle/style-info.css",
        SourceKind::StyleSheet,
    );
    // A custom scheme no transport serves resolves while it is held.
    resources
        .shared
        .transports
        .resolve(
            "/page/card.web.bundle/root.js",
            resources.base_url().as_ref(),
        )
        .expect("a held URL resolves");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), module.changed())
            .await
            .is_err(),
        "the load waits while the install runs"
    );

    let bytecode = load(
        &resources,
        "app:///page/card.web.bundle/chunk.js",
        SourceKind::Script,
    );
    release.send(()).unwrap();
    let Ok(LoadedSource::Module { source, .. }) = result(module).await else {
        panic!("the held load answers what the install registered");
    };
    assert_eq!(source, "export default 2;");
    let Ok(LoadedSource::ModuleBytecode { bytes, url }) = result(bytecode).await else {
        panic!("the held load answers the module bytecode the install registered");
    };
    assert_eq!(bytes.as_ref(), b"module bytecode");
    assert_eq!(url, "app:///page/card.web.bundle/chunk.js");
    assert!(matches!(
        result(sheet).await,
        Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(_)))
    ));
    // Released: what the install registered is an ordinary registration now.
    assert!(
        resources
            .shared
            .transports
            .installs
            .install_covering(&Url::parse("app:///page/card.web.bundle/root.js").unwrap())
            .is_none()
    );
}

/// A load that missed the registry while an install was registering its
/// container, and looks for the install after it ended: no hold covers the
/// URL, and a stylesheet the install registered pre-parsed is still this
/// load's answer, never a fetch of bytes it does not have. A script with no
/// hold goes on to its fetch.
#[tokio::test]
async fn a_load_past_an_ended_install_answers_its_pre_parsed_stylesheet() {
    let resources = resources();
    let sheet = Url::parse("app:///page/card.web.bundle/style-info.css").unwrap();
    resources
        .register_style_sheet(
            sheet.as_str(),
            Arc::new(bobcat_core::PreparsedStyleSheet::default()),
        )
        .unwrap();
    assert!(
        resources
            .shared
            .transports
            .installs
            .install_covering(&sheet)
            .is_none()
    );

    assert!(matches!(
        after_install(&resources.shared, &sheet, SourceKind::StyleSheet).await,
        Some(Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(_))))
    ));
    let script = Url::parse("app:///page/card.web.bundle/root.js").unwrap();
    assert!(
        after_install(&resources.shared, &script, SourceKind::Script)
            .await
            .is_none()
    );
}

/// An install that fails fails every load it held, with its own message,
/// and leaves a note.
#[tokio::test]
async fn a_failed_install_fails_the_loads_it_held() {
    let resources = resources();
    let release = install_held(
        &resources,
        "app:///page/card.web.bundle",
        Err("the bundle would not decode".to_owned()),
    );
    let module = load(
        &resources,
        "app:///page/card.web.bundle/root.js",
        SourceKind::Script,
    );
    release.send(()).unwrap();

    let Err(LynxViewError::Resource(error)) = result(module).await else {
        panic!("the install's failure is the load's");
    };
    assert_eq!(error.kind, ResourceErrorKind::ResponseBody);
    assert!(
        error.message.contains("would not decode"),
        "{}",
        error.message
    );
    assert!(
        resources
            .take_notes()
            .iter()
            .any(|note| note.contains("would not decode"))
    );
}

/// What an install covers is its container's path and what is under it,
/// whatever the query and fragment: the BTS boot at `U#background-thread`
/// is held, a sibling path that merely starts with the same text is not.
#[test]
fn an_install_holds_its_container_path_and_nothing_beside_it() {
    let resources = resources();
    let release = install_held(&resources, "app:///page/card.web.bundle?v=1", Ok(()));
    let held = |url: &str| {
        resources
            .shared
            .transports
            .installs
            .install_covering(&Url::parse(url).unwrap())
            .is_some()
    };
    assert!(held("app:///page/card.web.bundle?v=1"));
    assert!(held("app:///page/card.web.bundle?v=1#background-thread"));
    assert!(held("app:///page/card.web.bundle/root.js?v=1"));
    assert!(held(
        "app:///page/card.web.bundle/custom_section/CSS.css?v=1"
    ));
    assert!(!held("app:///page/card.web.bundle2/root.js"));
    assert!(!held("app:///page/other.js"));
    assert!(!held("other:///page/card.web.bundle/root.js"));
    drop(release);
}

/// The code cache's Store side. The memory tier is the process's own
/// (`Artifacts::global`), which every test in this binary shares, so every
/// test here uses script text no other test delivers and asserts only about
/// its own keys.
mod code_cache {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;

    use bytes::Bytes;
    use vant::artifact::{
        ArtifactCache, ArtifactConfig, ArtifactKey, Artifacts, SourceDigest, Variant,
    };

    use super::*;
    use crate::{DiskCacheConfig, NativePlatform, flush_code_cache};

    /// Serializes the tests whose system has a code store, each of which
    /// flushes at least when it drops: a flush takes the whole process's
    /// queue, so one test's flush could write another's artifact to its own
    /// directory.
    static FLUSHES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bobcat-resources-code-cache-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn disk(dir: &Path) -> DiskCacheConfig {
        DiskCacheConfig {
            dir: dir.to_owned(),
            budget_bytes: 1 << 20,
            code_budget_bytes: 1 << 20,
        }
    }

    fn disk_resources(dir: &Path) -> (Resources, Vec<String>) {
        let (platform, notes) = NativePlatform::new(Some(&disk(dir)));
        let resources = Resources::with_platform(
            ResourcesConfig {
                base_url: Some(Url::parse("app:///page/main.js").unwrap()),
                log_to_stderr: false,
                ..ResourcesConfig::default()
            },
            platform,
            notes.clone(),
            || {},
        );
        (resources, notes)
    }

    /// Script text over the cache's threshold that only `test` delivers.
    fn script_text(test: &str) -> String {
        format!(
            "globalThis.{} = true;\n{}",
            test.replace('-', "_"),
            "// padding\n".repeat(128)
        )
    }

    fn key_of(text: &str, variant: &str) -> ArtifactKey {
        ArtifactKey {
            source: SourceDigest::of(text.as_bytes()),
            variant: Variant::of(&[b"test", variant.as_bytes()]),
        }
    }

    async fn deliver(resources: &Resources, url: &str, text: &str) {
        resources
            .register(url, text.to_owned(), Some("text/javascript"))
            .unwrap();
        match result(load(resources, url, SourceKind::Script))
            .await
            .unwrap()
        {
            LoadedSource::Module { source, .. } => assert_eq!(source, text),
            source => panic!("expected a script, got {source:?}"),
        }
    }

    /// Writes `key`'s artifact into the code cache under `dir` the way a
    /// flush would, without the process's tier.
    fn plant(dir: &Path, key: ArtifactKey, artifact: &'static [u8]) {
        let memory = Arc::new(Artifacts::new(1 << 20));
        memory.insert(
            bobcat_core::script_engine_id(),
            key,
            Bytes::from_static(artifact),
        );
        let cache = ArtifactCache::open(
            memory,
            crate::FsStore::open(dir.join("code")).unwrap(),
            bobcat_core::script_engine_id(),
            ArtifactConfig::default(),
            std::time::SystemTime::now(),
        );
        assert_eq!(cache.flush(std::time::SystemTime::now()), 1);
    }

    /// Whether the code cache under `dir` holds `key`, read through a tier
    /// of its own.
    fn stored(dir: &Path, key: ArtifactKey) -> bool {
        let memory = Arc::new(Artifacts::new(1 << 20));
        let cache = ArtifactCache::open(
            Arc::clone(&memory),
            crate::FsStore::open(dir.join("code")).unwrap(),
            bobcat_core::script_engine_id(),
            ArtifactConfig::default(),
            std::time::SystemTime::now(),
        );
        cache.preload(key.source, std::time::SystemTime::now());
        memory.contains(bobcat_core::script_engine_id(), key)
    }

    #[tokio::test]
    async fn a_scripts_bytecode_survives_into_a_second_resource_system_over_the_same_cache_directory()
     {
        let _flushes = FLUSHES.lock().await;
        let dir = TempDir::new();
        let text = script_text("survives-into-a-second-system");
        let key = key_of(&text, "survives");
        let engine = bobcat_core::script_engine_id();
        let global = Artifacts::global();
        {
            let (resources, _) = disk_resources(&dir.0);
            deliver(&resources, "app:///page/big.js", &text).await;
            // What the engine does after compiling the delivered text.
            global.insert(engine, key, Bytes::from_static(b"compiled"));
            flush_code_cache(&resources);
            let code = resources.shared.code.as_ref().expect("a code cache");
            tokio::time::timeout(Duration::from_secs(10), async {
                while code.flushing.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the flush ends");
            assert!(stored(&dir.0, key), "the flush wrote the artifact");
        }
        global.remove(engine, key);

        let (resources, _) = disk_resources(&dir.0);
        assert!(!global.contains(engine, key));
        deliver(&resources, "app:///page/big.js", &text).await;
        assert_eq!(
            global.get(engine, key).as_deref(),
            Some(&b"compiled"[..]),
            "delivering the text preloaded its artifact"
        );
        global.remove(engine, key);
    }

    #[tokio::test]
    async fn only_the_delivered_texts_artifacts_are_preloaded() {
        let _flushes = FLUSHES.lock().await;
        let dir = TempDir::new();
        let delivered = script_text("only-the-delivered-text-delivered");
        let other = script_text("only-the-delivered-text-other");
        let (delivered_key, other_key) = (key_of(&delivered, "a"), key_of(&other, "a"));
        plant(&dir.0, delivered_key, b"delivered");
        plant(&dir.0, other_key, b"other");
        let engine = bobcat_core::script_engine_id();

        let (resources, _) = disk_resources(&dir.0);
        deliver(&resources, "app:///page/delivered.js", &delivered).await;
        assert!(Artifacts::global().contains(engine, delivered_key));
        assert!(!Artifacts::global().contains(engine, other_key));
        Artifacts::global().remove(engine, delivered_key);
    }

    #[tokio::test]
    async fn a_script_under_the_threshold_is_not_preloaded() {
        let _flushes = FLUSHES.lock().await;
        let dir = TempDir::new();
        let text = "globalThis.a_script_under_the_threshold = true;";
        assert!(text.len() < bobcat_core::MIN_CACHED_SOURCE_BYTES);
        let key = key_of(text, "small");
        plant(&dir.0, key, b"small");

        let (resources, _) = disk_resources(&dir.0);
        deliver(&resources, "app:///page/small.js", text).await;
        assert!(!Artifacts::global().contains(bobcat_core::script_engine_id(), key));
    }

    #[tokio::test]
    async fn without_a_disk_cache_there_is_no_code_store_and_delivery_is_unchanged() {
        let resources = resources();
        assert!(resources.shared.code.is_none());
        let text = script_text("without-a-disk-cache");
        deliver(&resources, "app:///page/plain.js", &text).await;
        flush_code_cache(&resources);
    }

    #[tokio::test]
    async fn a_code_directory_that_cannot_be_opened_is_a_note_and_the_system_runs() {
        let dir = TempDir::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        // A file where the directory would be.
        std::fs::write(dir.0.join("code"), b"not a directory").unwrap();

        let (resources, notes) = disk_resources(&dir.0);
        assert!(
            notes.iter().any(|note| note.contains("the code cache at")),
            "{notes:?}"
        );
        assert!(
            !notes.iter().any(|note| note.contains("the disk cache at")),
            "the byte tier opened: {notes:?}"
        );
        assert!(resources.shared.code.is_none());
        let text = script_text("a-code-directory-that-cannot-be-opened");
        deliver(&resources, "app:///page/runs.js", &text).await;
    }

    /// A platform whose code cache is an in-memory [`vant::AsyncStore`]:
    /// what the browser's Cache API store is to the resource system.
    mod async_store {
        use std::collections::HashMap;
        use std::time::SystemTime;

        use vant::{
            ArtifactStore, AsyncStore, Estimate, Meta, NoStore, Parts, Platform, StoreError, Tier,
            Volatility,
        };

        use super::*;
        use crate::{NativeDecoder, NativeFetch};

        /// Clones share their entries. A put waits until `puts_open` is
        /// `true`, so a test can keep a flush in flight.
        #[derive(Clone)]
        struct MemAsyncStore {
            entries: Arc<Mutex<HashMap<String, (Meta, Bytes)>>>,
            puts_open: watch::Sender<bool>,
            puts: Arc<AtomicUsize>,
        }

        impl MemAsyncStore {
            fn new() -> Self {
                Self {
                    entries: Arc::default(),
                    puts_open: watch::Sender::new(true),
                    puts: Arc::default(),
                }
            }

            fn holds(&self, name: &str) -> bool {
                self.entries.lock().unwrap().contains_key(name)
            }
        }

        impl AsyncStore for MemAsyncStore {
            fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + Send {
                let metas = self
                    .entries
                    .lock()
                    .unwrap()
                    .values()
                    .map(|(meta, _)| meta.clone())
                    .collect();
                std::future::ready(Ok(metas))
            }

            fn get(
                &self,
                key: &str,
            ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + Send
            {
                std::future::ready(Ok(self.entries.lock().unwrap().get(key).cloned()))
            }

            fn put(
                &self,
                meta: &Meta,
                body: Bytes,
            ) -> impl Future<Output = Result<(), StoreError>> + Send {
                let (store, meta) = (self.clone(), meta.clone());
                let mut open = self.puts_open.subscribe();
                async move {
                    let _ = open.wait_for(|open| *open).await;
                    store.puts.fetch_add(1, Ordering::Relaxed);
                    store
                        .entries
                        .lock()
                        .unwrap()
                        .insert(meta.key.clone(), (meta, body));
                    Ok(())
                }
            }

            fn delete(&self, key: &str) -> impl Future<Output = Result<(), StoreError>> + Send {
                self.entries.lock().unwrap().remove(key);
                std::future::ready(Ok(()))
            }

            fn touch(
                &self,
                key: &str,
                now: SystemTime,
            ) -> impl Future<Output = Result<(), StoreError>> + Send {
                if let Some(entry) = self.entries.lock().unwrap().get_mut(key) {
                    entry.0.last_used = now;
                }
                std::future::ready(Ok(()))
            }

            fn estimate(&self) -> impl Future<Output = Estimate> + Send {
                std::future::ready(Estimate::default())
            }

            fn volatility(&self) -> Volatility {
                Volatility::EvictableAnytime
            }
        }

        struct AsyncPlatform(MemAsyncStore);

        impl Platform for AsyncPlatform {
            type Fetch = NativeFetch;
            type Store = NoStore;
            type AsyncStore = MemAsyncStore;
            type Decoder = NativeDecoder;

            fn into_parts(self) -> Parts<NativeFetch, NoStore, MemAsyncStore, NativeDecoder> {
                Parts {
                    fetch: NativeFetch,
                    decoder: NativeDecoder,
                    bytes: None,
                    artifacts: Some(Tier {
                        store: ArtifactStore::Async(self.0),
                        budget: 1 << 20,
                    }),
                }
            }
        }

        fn async_resources(store: &MemAsyncStore) -> Resources<AsyncPlatform> {
            Resources::with_platform(
                ResourcesConfig {
                    base_url: Some(Url::parse("app:///page/main.js").unwrap()),
                    log_to_stderr: false,
                    ..ResourcesConfig::default()
                },
                AsyncPlatform(store.clone()),
                Vec::new(),
                || {},
            )
        }

        /// Delivers `text` as the script at `url`, and answers once its
        /// completion has arrived.
        async fn deliver(resources: &Resources<AsyncPlatform>, url: &str, text: &str) {
            resources
                .register(url, text.to_owned(), Some("text/javascript"))
                .unwrap();
            let (sender, response) = watch::channel(None);
            start(
                resources,
                Url::parse(url).unwrap(),
                SourceKind::Script,
                Destination::Cache(sender),
            );
            match result(response).await.unwrap() {
                LoadedSource::Module { source, .. } => assert_eq!(source, text),
                source => panic!("expected a script, got {source:?}"),
            }
        }

        async fn flush_ended(resources: &Resources<AsyncPlatform>) {
            let code = resources.shared.code.as_ref().expect("a code cache");
            tokio::time::timeout(Duration::from_secs(10), async {
                while code.flushing.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the flush ends");
        }

        fn name(key: ArtifactKey) -> String {
            format!(
                "{:016x}/{:032x}/{:016x}",
                bobcat_core::script_engine_id().0,
                key.source.0,
                key.variant.0
            )
        }

        #[tokio::test]
        async fn bytecode_a_flush_wrote_is_admitted_before_the_script_is_delivered() {
            let _flushes = FLUSHES.lock().await;
            let store = MemAsyncStore::new();
            let text = script_text("async-store-admitted-before-delivery");
            let key = key_of(&text, "async");
            let engine = bobcat_core::script_engine_id();
            let global = Artifacts::global();
            {
                let resources = async_resources(&store);
                deliver(&resources, "app:///page/async.js", &text).await;
                global.insert(engine, key, Bytes::from_static(b"compiled"));
                flush_code_cache(&resources);
                flush_ended(&resources).await;
                assert!(store.holds(&name(key)), "the flush wrote the artifact");
            }
            global.remove(engine, key);

            let resources = async_resources(&store);
            deliver(&resources, "app:///page/async.js", &text).await;
            assert_eq!(
                global.get(engine, key).as_deref(),
                Some(&b"compiled"[..]),
                "the preload ran before the completion was delivered"
            );
            global.remove(engine, key);
        }

        #[tokio::test]
        async fn a_turn_starts_no_flush_while_one_is_in_flight() {
            let _flushes = FLUSHES.lock().await;
            let store = MemAsyncStore::new();
            let engine = bobcat_core::script_engine_id();
            let global = Artifacts::global();
            let resources = async_resources(&store);
            let (first, second) = (
                key_of(&script_text("async-store-one-flush-first"), "async"),
                key_of(&script_text("async-store-one-flush-second"), "async"),
            );

            store.puts_open.send_replace(false);
            global.insert(engine, first, Bytes::from_static(b"first"));
            flush_code_cache(&resources);
            let code = resources.shared.code.as_ref().unwrap();
            assert!(code.flushing.load(Ordering::Acquire));
            // The flush task has taken the queue once nothing is pending; its
            // put then waits.
            tokio::time::timeout(Duration::from_secs(10), async {
                while global.has_pending() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the flush takes the queue");
            global.insert(engine, second, Bytes::from_static(b"second"));
            // The next turns, while the first flush waits on its put.
            for _ in 0..3 {
                flush_code_cache(&resources);
            }
            assert!(global.has_pending(), "the second artifact is still queued");
            store.puts_open.send_replace(true);
            flush_ended(&resources).await;
            assert_eq!(store.puts.load(Ordering::Relaxed), 1);
            assert!(store.holds(&name(first)) && !store.holds(&name(second)));

            // The turn after it ends starts the next flush.
            flush_code_cache(&resources);
            flush_ended(&resources).await;
            assert_eq!(store.puts.load(Ordering::Relaxed), 2);
            assert!(store.holds(&name(second)));
            global.remove(engine, first);
            global.remove(engine, second);
        }
    }
}

/// Registered module bytecode answers a module request with its bytes and
/// URL, and every
/// other kind of request for its URL fails, as for a pre-parsed stylesheet:
/// nothing but a module request reads it.
#[tokio::test]
async fn registered_module_bytecode_answers_only_a_module_request() {
    let resources = resources();
    let url = "app:///page/compiled.js";
    let registered = resources
        .register_module_bytecode(url, b"module bytecode".to_vec())
        .unwrap();
    assert_eq!(registered.as_str(), url);

    let Ok(LoadedSource::ModuleBytecode {
        bytes,
        url: answered,
    }) = result(load(&resources, url, SourceKind::Script)).await
    else {
        panic!("a module request reads module bytecode");
    };
    assert_eq!(bytes.as_ref(), b"module bytecode");
    assert_eq!(answered, url);

    for (kind, name) in [
        (SourceKind::StyleSheet, "stylesheet"),
        (SourceKind::Font, "font"),
        (SourceKind::Fetch { timed: false }, "fetch"),
    ] {
        let Err(LynxViewError::Resource(error)) = result(load(&resources, url, kind)).await else {
            panic!("a {name} request does not read module bytecode");
        };
        assert_eq!(
            error.kind,
            ResourceErrorKind::UnsupportedOperation,
            "{name}"
        );
        assert!(
            error.message.contains("module bytecode"),
            "{}",
            error.message
        );
    }
}
