//! Times a view's boot from a `.web.bundle`'s bytes, decoded before the view
//! is created (`PageSource::from_bytes`) and decoded while it boots
//! (`PageSource::defer`): how long the embedder's thread takes until the view
//! exists, when the container's registration finished, and when the view
//! reported its boot done (`EngineEvent::ScriptFinished`).
//!
//! ```sh
//! cargo run --release -p bobcat-source --example boot_timing -- path/to/card.web.bundle
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, ScreenMetrics, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::{BundleInstaller, ContainerSources, PageSource};
use url::Url;

const RUNS: usize = 20;

fn resources(input: &Url) -> Resources {
    Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            container_installer: Some(Arc::new(BundleInstaller)),
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    )
}

/// One boot: until the view exists, and until the container was registered.
struct Boot {
    view: Duration,
    registered: Duration,
    finished: Duration,
}

async fn boot(input: &Url, bytes: &[u8], deferred: bool) -> Boot {
    let screen = ScreenMetrics::for_viewport(393.0, 727.0, 1.0);
    let start = Instant::now();
    let resources = resources(input);
    let registered = Arc::new(Mutex::new(None));
    let sources = if deferred {
        let page = PageSource::defer(input, bytes.to_vec()).expect("the card defers");
        let sources = page.view_sources(screen);
        // The install `DeferredPage::register_in_background` runs, with the
        // time it finished recorded.
        let (url, bytes, done) = (input.clone(), bytes.to_vec(), Arc::clone(&registered));
        resources.register_in_background(input, move |registrar| {
            let result = ContainerSources::from_bytes(&url, &bytes)
                .map_err(|error| error.to_string())?
                .register_through(registrar)
                .map_err(|error| error.to_string());
            *done.lock().unwrap() = Some(start.elapsed());
            result
        });
        sources
    } else {
        let page = PageSource::from_bytes(input, bytes).expect("the card decodes");
        page.register_with(&resources);
        *registered.lock().unwrap() = Some(start.elapsed());
        page.view_sources(screen)
    };
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .expect("a group");
    let mut view = group
        .create_lynx_view(393.0, 727.0, 1.0, resources.builder(), Vec::new(), sources)
        .expect("a view");
    let view_at = start.elapsed();
    // Boot's first flush commits a frame, so the view is turned the way the
    // CLI turns it: an offscreen painter posts frames while the view pumps.
    let mut painter = Painter::new(DrawTarget::Offscreen, 393.0, 727.0, 1.0)
        .await
        .expect("a painter");
    painter.attach(&view).expect("the painter attaches");
    let mut finished = None;
    let booted = |events: Vec<EngineEvent>, finished: &mut Option<Duration>| {
        if let Some(event) = events.iter().find(|event| event.is_fatal()) {
            panic!("the view ended: {event:?}");
        }
        if events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished))
        {
            finished.get_or_insert(start.elapsed());
        }
    };
    let finished = loop {
        booted(view.pump(), &mut finished);
        let posted = painter.post_frame().expect("a frame posts");
        while !painter.frame_serviced(&posted) && finished.is_none() {
            tokio::time::sleep(Duration::from_micros(100)).await;
            booted(view.pump(), &mut finished);
        }
        painter.draw_posted(posted, false).expect("the frame draws");
        if let Some(finished) = finished {
            break finished;
        }
        tokio::time::sleep(Duration::from_micros(100)).await;
    };
    let registered_at = loop {
        if let Some(at) = *registered.lock().unwrap() {
            break at;
        }
        tokio::time::sleep(Duration::from_micros(50)).await;
    };
    drop(painter);
    drop(view);
    Boot {
        view: view_at,
        registered: registered_at,
        finished,
    }
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort();
    values[values.len() / 2]
}

fn main() {
    let path = std::env::args().nth(1).expect("a .web.bundle path");
    let bytes = std::fs::read(&path).expect("the bundle reads");
    let input = Url::from_file_path(std::fs::canonicalize(&path).unwrap()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for deferred in [false, true] {
        // One boot to warm the process (fonts, the GPU-free engine setup).
        runtime.block_on(boot(&input, &bytes, deferred));
        let boots: Vec<Boot> = (0..RUNS)
            .map(|_| runtime.block_on(boot(&input, &bytes, deferred)))
            .collect();
        let hidden = boots.iter().filter(|b| b.registered <= b.view).count();
        println!(
            "{:<8} medians: view created {:>9.1?}, container registered {:>9.1?}, \
             boot finished {:>9.1?}; registered before the view existed in {hidden}/{RUNS}",
            if deferred { "deferred" } else { "decoded" },
            median(boots.iter().map(|b| b.view).collect()),
            median(boots.iter().map(|b| b.registered).collect()),
            median(boots.iter().map(|b| b.finished).collect()),
        );
    }
}
