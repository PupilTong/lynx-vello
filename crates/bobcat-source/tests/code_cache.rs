//! The code cache end to end: a `ReactLynx` card booted over a disk cache
//! leaves its scripts' bytecode on disk, and a second resource system over
//! the same directory boots it from that bytecode to the same frame.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads,
};
use bobcat_resources::{DiskCacheConfig, NativePlatform, Resources, ResourcesConfig};
use bobcat_source::PageSource;
use serde_json::json;
use url::Url;
use vant::artifact::Artifacts;

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(120.0, 120.0, 1.0);

const MARKER: &str = "props-render green 3 retained 3 3";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How many entries the code cache under `dir` holds: the bodies whose
/// record is beside them. Counted from the directory listing rather than
/// through a second `FsStore`, whose scan sweeps a body with no record yet
/// and so would delete an entry the cache is in the middle of writing.
fn stored_entries(dir: &Path) -> usize {
    let Ok(items) = std::fs::read_dir(dir.join("code")) else {
        return 0;
    };
    items
        .filter_map(Result::ok)
        .map(|item| item.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "bin")
                && path.with_extension("json").is_file()
        })
        .count()
}

/// Whether everything the process's tier holds is on disk under `dir`. The
/// tier is this test's alone and holds only what the card compiled or what
/// was preloaded from `dir`, so once nothing is queued and the Store holds
/// as many entries as the tier, the last flush has finished writing.
fn persisted(dir: &Path) -> bool {
    let global = Artifacts::global();
    let stored = stored_entries(dir);
    !global.has_pending() && stored > 0 && stored >= global.len()
}

/// Boots the card over a disk cache in `dir`, waits for its first frame and
/// for the code cache to hold what it compiled, and answers the frame's
/// pixels.
async fn boot(dir: &Path) -> Vec<u8> {
    let input = Url::parse("app:///react-global-props.lynx.bundle").unwrap();
    let page = PageSource::from_native_bundle(
        &input,
        fixtures::fixture("react-global-props").page,
        "react-global-props__main-thread",
    )
    .unwrap();
    let (platform, notes) = NativePlatform::new(Some(&DiskCacheConfig {
        dir: dir.to_owned(),
        budget_bytes: 16 << 20,
        code_budget_bytes: 64 << 20,
    }));
    assert!(
        !notes.iter().any(|note| note.contains(" cache at ")),
        "{notes:?}"
    );
    let resources = Resources::with_platform(
        ResourcesConfig {
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        platform,
        notes,
        || {},
    );
    page.register_with(&resources);
    resources.set_base_url(Some(input));
    let mut sources = page.view_sources(SCREEN, "en-US");
    sources.global_props = Some(json!({"seed":3,"color":"green","keep":"retained"}).to_string());
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(120.0, 120.0, 1.0, resources.builder(), Vec::new(), sources)
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 120.0, 120.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let mut messages = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {}
                EngineEvent::ConsoleMessage { message, .. } => messages.push(message),
                other => panic!("the card failed: {other:?}"),
            }
        }
        painter.pump().unwrap();
        let shot = match painter.capture() {
            Ok(shot) => Some(shot),
            Err(EngineError::Render(message))
                if message == "no frame has been rendered to capture" =>
            {
                None
            }
            Err(error) => panic!("capture failed: {error}"),
        };
        let green = shot.as_ref().is_some_and(|shot| {
            shot.pixels[4 * (20 * 120 + 20)..4 * (20 * 120 + 21)] == [0, 128, 0, 255]
        });
        if green
            && view.is_ready()
            && messages.iter().any(|message| message == MARKER)
            && persisted(dir)
        {
            return shot.unwrap().pixels;
        }
        assert!(
            Instant::now() < deadline,
            "the card did not render and persist its bytecode; messages={messages:?}, stored={}, resident={}",
            stored_entries(dir),
            Artifacts::global().len()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_card_boots_from_the_bytecode_an_earlier_resource_system_left_on_disk() {
    let dir = TempDir(
        std::env::temp_dir().join(format!("bobcat-source-code-cache-{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&dir.0);
    let global = Artifacts::global();
    // This binary runs this one test, so the process's tier is this test's.
    global.clear();

    let first = boot(&dir.0).await;
    let stored = stored_entries(&dir.0);
    assert!(
        stored >= 2,
        "the main-thread and background scripts, at least"
    );

    // Nothing resident: whatever the second boot reads, it read from disk.
    global.clear();
    let second = boot(&dir.0).await;
    assert_eq!(first, second, "the same frame from bytecode as from text");
    assert!(
        !global.is_empty(),
        "delivering the card's scripts preloaded their bytecode"
    );
    assert!(
        global.take_pending().is_empty(),
        "every cached script was read rather than compiled"
    );
    assert_eq!(stored_entries(&dir.0), stored);
}
