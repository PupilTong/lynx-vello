//! Compiled `ReactLynx` entries use the actual source registry and both `QuickJS`
//! threads. A framework error report fails these tests even if boot resolves.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use url::Url;

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(393.0, 727.0, 1.0);

async fn boot(bytes: &[u8], name: &str) {
    let input = Url::parse(&format!("app:///{name}")).unwrap();
    let page = if name == "react-native.lynx.bundle" {
        PageSource::from_native_bundle(&input, bytes, "react-native__main-thread")
    } else {
        PageSource::from_bytes(&input, bytes)
    }
    .expect("decode compiled card");
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    boot_registered(page.view_sources(SCREEN, "en-US"), resources, name).await;
}

async fn boot_registered(sources: bobcat_core::ViewSources, resources: Resources, name: &str) {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(393.0, 727.0, 1.0, resources.builder(), Vec::new(), sources)
        .unwrap();
    let mut painter =
        bobcat_core::Painter::new(bobcat_core::DrawTarget::Offscreen, 393.0, 727.0, 1.0)
            .await
            .unwrap();
    painter.attach(&view).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut finished = false;
    let mut finished_at = None;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {
                    finished = true;
                    finished_at = Some(Instant::now());
                }
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("{name} [{level}] {message}");
                }
                other => panic!("{name}: unexpected runtime event: {other:?}"),
            }
        }
        if finished_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(100)) {
            break;
        }
        assert!(Instant::now() < deadline, "{name}: BTS boot did not finish");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(finished);
}

#[tokio::test]
async fn real_web_cards_boot_without_lynx_core() {
    boot(
        fixtures::fixture("basic-bindtap").page,
        "basic-bindtap.web.bundle",
    )
    .await;
    boot(
        fixtures::fixture("basic-class-selector").page,
        "basic-class-selector.web.bundle",
    )
    .await;
}

#[tokio::test]
async fn real_native_card_boots_without_lynx_core() {
    boot(
        fixtures::fixture("react-native").page,
        "react-native.lynx.bundle",
    )
    .await;
}

#[tokio::test]
async fn real_native_background_runs_from_its_preserved_custom_section() {
    let bytes = fixtures::fixture("react-native").page;
    let page = PageSource::from_native_bundle(
        &Url::parse("app:///custom-section-native.lynx.bundle").unwrap(),
        bytes,
        "react-native__main-thread",
    )
    .unwrap();
    let template = bobcat_source::native::decode(bytes).unwrap();
    let key = template
        .custom_sections
        .as_ref()
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .find(|key| key.contains("background."))
        .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let mut sources = page.view_sources(SCREEN, "en-US");
    let entry = serde_json::to_string(sources.background_entry.as_ref().unwrap()).unwrap();
    let key = serde_json::to_string(key).unwrap();
    let bootstrap = format!(
        r"
        import {{lynx}} from 'bobcat:bts-runtime';
        const original = lynx.requireModule;
        let loaded = false;
        lynx.requireModule = function(path, ...args) {{
            if (path !== '/app-service.js') return original(path, ...args);
            lynx.requireModule = original;
            loaded = true;
            return lynx.loadScript({key}, {{}});
        }};
        await import({entry});
        if (!loaded) throw Error('custom-section bootstrap did not run');
    "
    );
    resources
        .register(
            "app:///custom-bts.js",
            bootstrap.into_bytes(),
            Some("text/javascript"),
        )
        .unwrap();
    sources.background_entry = Some("app:///custom-bts.js".to_owned());
    boot_registered(sources, resources, "native custom-section BTS").await;
}

#[tokio::test]
async fn compiled_bindtap_set_state_changes_the_painted_pixels_twice() {
    paint_and_tap(
        fixtures::fixture("basic-bindtap").page,
        [50, 50],
        &[[255, 192, 203, 255], [0, 128, 0, 255], [255, 192, 203, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_worklets_and_main_thread_refs_cross_both_directions() {
    paint_and_tap(
        fixtures::fixture("basic-mts-run-on-main-thread").page,
        [150, 150],
        &[[0, 128, 0, 255]],
        None,
    )
    .await;
    paint_and_tap(
        fixtures::fixture("basic-mts-run-on-background").page,
        [50, 50],
        &[[255, 192, 203, 255], [0, 128, 0, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_cards_share_data_across_their_group() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, LynxView, Painter};

    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const ORANGE: [u8; 4] = [255, 165, 0, 255];

    async fn until_painted<F: bobcat_core::resource::ResourceFetcher + 'static>(
        cards: &mut [(LynxView<F>, Painter, Resources)],
        index: usize,
        colour: [u8; 4],
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for (view, painter, _) in cards.iter_mut() {
                for event in view.pump() {
                    match event {
                        EngineEvent::ScriptFinished => {}
                        EngineEvent::ConsoleMessage { level, message, .. } => {
                            eprintln!("[{level}] {message}");
                        }
                        other => panic!("{other:?}"),
                    }
                }
                painter.pump().unwrap();
            }
            let offset = (50 * 240 + 50) * 4;
            let pixel = match cards[index].1.capture() {
                Ok(shot) => Some(<[u8; 4]>::try_from(&shot.pixels[offset..offset + 4]).unwrap()),
                Err(bobcat_core::EngineError::Render(message))
                    if message == "no frame has been rendered to capture" =>
                {
                    None
                }
                Err(error) => panic!("capture failed: {error}"),
            };
            if pixel == Some(colour) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "card {index}: expected {colour:?}, got {pixel:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut cards = Vec::new();
    for name in ["react-set-shared-data", "react-get-shared-data"] {
        let input = Url::parse(&format!("app:///{name}.web.bundle")).unwrap();
        let page = PageSource::from_bytes(&input, fixtures::fixture(name).page).unwrap();
        let resources = Resources::new(ResourcesConfig::default(), || {});
        page.register_with(&resources);
        let view = group
            .create_lynx_view(
                240.0,
                240.0,
                1.0,
                resources.builder(),
                Vec::new(),
                page.view_sources(SCREEN, "en-US"),
            )
            .unwrap();
        let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
            .await
            .unwrap();
        painter.attach(&view).unwrap();
        cards.push((view, painter, resources));
    }
    until_painted(&mut cards, 0, GREEN).await;
    until_painted(&mut cards, 1, ORANGE).await;
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        cards[1].1.dispatch_input(InputEvent::pointer(
            Point2D::new(50.0, 50.0),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    until_painted(&mut cards, 1, GREEN).await;
}

#[tokio::test]
async fn compiled_bts_ref_queries_and_native_props_change_real_pixels() {
    paint_and_tap(
        fixtures::fixture("react-bts-query").page,
        [150, 150],
        &[[0, 128, 0, 255]],
        Some("react-bts-query verified"),
    )
    .await;
}

#[tokio::test]
async fn compiled_viewpager_select_tab_turns_to_the_fourth_page() {
    paint_and_tap(
        fixtures::fixture("react-viewpager").page,
        [120, 120],
        &[[0, 128, 0, 255], [255, 255, 0, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_swiper_scroll_to_turns_to_the_fourth_item() {
    paint_and_tap(
        fixtures::fixture("react-swiper-scroll-to").page,
        [120, 120],
        &[[0, 128, 0, 255], [255, 255, 0, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_dialog_opens_modal_and_reports_close() {
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    let page = fixtures::fixture("react-dialog").page;
    paint_and_tap(page, [120, 120], &[RED, GREEN, BLUE], None).await;
    paint_and_tap(page, [236, 236], &[RED, YELLOW, MAGENTA], None).await;
}

#[tokio::test]
async fn compiled_overlay_shows_its_panel_and_reports_show_and_dismiss() {
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    const ORANGE: [u8; 4] = [255, 165, 0, 255];
    let page = fixtures::fixture("react-overlay").page;
    paint_and_tap(page, [60, 60], &[RED, GREEN, BLUE], None).await;
    paint_and_tap(page, [180, 180], &[RED, MAGENTA], None).await;
    paint_and_tap(page, [220, 220], &[RED, ORANGE], None).await;
}

#[tokio::test]
async fn compiled_pass_through_overlay_lets_a_tap_outside_its_content_through() {
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CYAN: [u8; 4] = [0, 255, 255, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    let page = fixtures::fixture("react-overlay-pass-through").page;
    paint_and_tap(page, [210, 210], &[BLUE, CYAN, MAGENTA], None).await;
}

#[tokio::test]
async fn compiled_svg_component_draws_content_and_src() {
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    paint_and_sample(
        fixtures::fixture("react-svg").page,
        &[
            (60, 54, GREEN),
            (60, 25, YELLOW),
            (124, 4, BLUE),
            (40, 80, YELLOW),
            (6, 64, BLUE),
            (110, 80, WHITE),
            (60, 128, MAGENTA),
            (60, 180, YELLOW),
            (124, 124, GREEN),
            (135, 135, YELLOW),
        ],
    )
    .await;
}

#[tokio::test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "only ImageIO answers an animated GIF's frames"
)]
async fn compiled_animated_image_plays_its_own_loops_and_answers_its_methods() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    let page = PageSource::from_bytes(
        &Url::parse("app:///animated.web.bundle").unwrap(),
        fixtures::fixture("react-animated-image").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let view = group
        .create_lynx_view(
            240.0,
            240.0,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    let mut card = AnimatedCard {
        view,
        painter,
        booted: false,
        logged: Vec::new(),
    };

    card.until_status(GREEN).await;
    card.idle(Duration::from_millis(800)).await;
    assert_eq!(
        card.of("playing"),
        [
            "startplay {}",
            r#"load {"width":8,"height":8}"#,
            "currentloopcomplete {}",
            "currentloopcomplete {}",
            "currentloopcomplete {}",
            "finalloopcomplete {}",
        ],
        "three loops, not the file's two"
    );
    assert_eq!(card.of("paused"), [r#"load {"width":8,"height":8}"#]);

    card.logged.clear();
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        card.painter.dispatch_input(InputEvent::pointer(
            Point2D::new(
                f32::from(AnimatedCard::STATUS.0),
                f32::from(AnimatedCard::STATUS.1),
            ),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    card.until_status(BLUE).await;
    assert_eq!(
        card.of("playing"),
        [
            "startAnimate 0 Animation restarted.",
            "currentloopcomplete {}",
            "currentloopcomplete {}",
            "currentloopcomplete {}",
            "finalloopcomplete {}",
        ]
    );
    assert_eq!(card.of("paused"), ["stopAnimation 0 Animation stopped."]);
}

struct AnimatedCard {
    view: bobcat_core::LynxView<bobcat_resources::ViewResources>,
    painter: bobcat_core::Painter,
    booted: bool,
    logged: Vec<String>,
}

impl AnimatedCard {
    const STATUS: (u16, u16) = (120, 180);

    fn turn(&mut self) {
        for event in self.view.pump() {
            match event {
                EngineEvent::ScriptFinished => self.booted = true,
                EngineEvent::ConsoleMessage { message, .. } => {
                    if let Some(line) = message.strip_prefix("animated-image ") {
                        self.logged.push(line.to_owned());
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        self.painter.pump().unwrap();
    }

    async fn until_status(&mut self, colour: [u8; 4]) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.turn();
            if self.booted {
                let shot = self.painter.capture().unwrap();
                let at = (usize::from(Self::STATUS.1) * 240 + usize::from(Self::STATUS.0)) * 4;
                if shot.pixels[at..at + 4] == colour {
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "#status never turned {colour:?}; logged {:?}",
                self.logged
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn idle(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.turn();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn of(&self, id: &str) -> Vec<String> {
        self.logged
            .iter()
            .filter_map(|line| line.strip_prefix(&format!("{id} ")))
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "only macOS has a video player (AVFoundation)"
)]
async fn compiled_video_opens_paused_and_plays_on_tap() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const CLIP: (usize, usize) = (120, 90);
    const STATUS: (usize, usize) = (120, 210);

    let page = PageSource::from_bytes(
        &Url::parse("app:///video.web.bundle").unwrap(),
        fixtures::fixture("react-video").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let view = group
        .create_lynx_view(
            240.0,
            240.0,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    let mut card = LoggedCard {
        view,
        painter,
        prefix: "video ",
        booted: false,
        logged: Vec::new(),
    };

    card.until("the first frame drawn red", |card| {
        card.of("clip")
            .iter()
            .any(|line| line.starts_with("firstframe"))
            && card
                .pixel(CLIP)
                .is_some_and(|texel| near(texel, [255, 0, 0, 255]))
    })
    .await;
    card.idle(Duration::from_millis(600)).await;
    assert_eq!(card.of("clip"), [r#"firstframe {"duration":1.2}"#]);
    assert!(
        card.pixel(CLIP)
            .is_some_and(|texel| near(texel, [255, 0, 0, 255])),
        "a paused video stays on its first frame: {:?}",
        card.pixel(CLIP)
    );

    card.logged.clear();
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        card.painter.dispatch_input(InputEvent::pointer(
            Point2D::new(120.0, 210.0),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    card.until("#status green", |card| card.pixel(STATUS) == Some(GREEN))
        .await;
    card.idle(Duration::from_millis(400)).await;
    assert_eq!(
        card.of("clip"),
        [r#"play 0 {"success":true}"#, "ended {}"],
        "one play, one end"
    );
    assert!(
        card.pixel(CLIP)
            .is_some_and(|texel| near(texel, [0, 0, 255, 255])),
        "it rests on its last frame: {:?}",
        card.pixel(CLIP)
    );
}

fn near(texel: [u8; 4], expected: [u8; 4]) -> bool {
    texel
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 24)
}

struct LoggedCard {
    view: bobcat_core::LynxView<bobcat_resources::ViewResources>,
    painter: bobcat_core::Painter,
    prefix: &'static str,
    booted: bool,
    logged: Vec<String>,
}

impl LoggedCard {
    fn turn(&mut self) {
        for event in self.view.pump() {
            match event {
                EngineEvent::ScriptFinished => self.booted = true,
                EngineEvent::ConsoleMessage { message, .. } => {
                    if let Some(line) = message.strip_prefix(self.prefix) {
                        self.logged.push(line.to_owned());
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        self.painter.pump().unwrap();
    }

    fn pixel(&mut self, at: (usize, usize)) -> Option<[u8; 4]> {
        if !self.booted {
            return None;
        }
        let shot = match self.painter.capture() {
            Ok(shot) => shot,
            Err(bobcat_core::EngineError::Render(message))
                if message == "no frame has been rendered to capture" =>
            {
                return None;
            }
            Err(error) => panic!("capture failed: {error}"),
        };
        let index = (at.1 * 240 + at.0) * 4;
        shot.pixels[index..index + 4].try_into().ok()
    }

    async fn until(&mut self, what: &str, done: impl Fn(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.turn();
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what} never happened; logged {:?}",
                self.logged
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn idle(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.turn();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn of(&self, id: &str) -> Vec<String> {
        self.logged
            .iter()
            .filter_map(|line| line.strip_prefix(&format!("{id} ")))
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
async fn compiled_scroll_coordinator_folds_its_header_before_the_content() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};

    const HEIGHT: u16 = 400;
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];

    let page = PageSource::from_bytes(
        &Url::parse("app:///react-scroll-coordinator.web.bundle").unwrap(),
        fixtures::fixture("react-scroll-coordinator").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let (width, height) = (f32::from(COORDINATOR_WIDTH), f32::from(HEIGHT));
    let mut view = group
        .create_lynx_view(
            width,
            height,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let mut booted = false;
    let band = settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(150, 150, RED), (150, 250, GREEN), (150, 350, YELLOW)],
        "boot",
    );
    eprintln!("toolbar over the header at boot: {band:?}");
    let [red, green, blue, _] = band;
    assert!(
        (126..=129).contains(&red) && green == 0 && (126..=129).contains(&blue),
        "the translucent toolbar shows the red header through it, got {band:?}"
    );

    for (phase, y) in [
        (PointerPhase::Down, 350.0),
        (PointerPhase::Move, 202.0),
        (PointerPhase::Up, 202.0),
    ] {
        painter.dispatch_input(InputEvent::pointer(
            Point2D::new(150.0, y),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    let band = settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(150, 65, GREEN), (150, 150, GREEN), (150, 200, YELLOW)],
        "folded",
    );
    eprintln!("toolbar over the header folded: {band:?}");
    assert!(
        (126..=129).contains(&band[0]) && band[1] == 0 && (126..=129).contains(&band[2]),
        "folded, the toolbar covers the header's bottom band, got {band:?}"
    );
}

#[tokio::test]
async fn compiled_swipers_start_on_current() {
    use bobcat_core::{DrawTarget, Painter};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const CYAN: [u8; 4] = [0, 255, 255, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];

    let page = PageSource::from_bytes(
        &Url::parse("app:///react-swiper.web.bundle").unwrap(),
        fixtures::fixture("react-swiper").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let (width, height) = (f32::from(COORDINATOR_WIDTH), 360.0);
    let mut view = group
        .create_lynx_view(
            width,
            height,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let mut booted = false;
    settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[
            (60, 60, BLUE),
            (180, 60, GREEN),
            (60, 180, GREEN),
            (5, 180, RED),
            (73, 107, WHITE),
            (227, 60, WHITE),
            (180, 180, GREEN),
            (180, 227, WHITE),
            (60, 347, RED),
            (166, 347, CYAN),
            (180, 347, YELLOW),
        ],
        "boot",
    );
}

#[tokio::test]
async fn compiled_refresh_view_holds_a_pulled_header() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];

    let page = PageSource::from_bytes(
        &Url::parse("app:///react-refresh-view.web.bundle").unwrap(),
        fixtures::fixture("react-refresh-view").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let (width, height) = (f32::from(COORDINATOR_WIDTH), 360.0);
    let mut view = group
        .create_lynx_view(
            width,
            height,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let mut booted = false;
    let opened = [
        (75, 5, YELLOW),
        (75, 170, GREEN),
        (225, 5, YELLOW),
        (225, 170, GREEN),
        (75, 355, GREEN),
    ];
    settle(&mut view, &mut painter, &mut booted, &opened, "boot");

    let pull = |painter: &mut Painter, x: f32, phase: PointerPhase, y: f32| {
        painter.dispatch_input(InputEvent::pointer(
            Point2D::new(x, y),
            1,
            PointerKind::Touch,
            phase,
        ));
    };
    pull(&mut painter, 75.0, PointerPhase::Down, 100.0);
    pull(&mut painter, 75.0, PointerPhase::Move, 250.0);
    let held = [(75, 95, RED), (75, 140, RED), (75, 150, YELLOW)];
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        settle(&mut view, &mut painter, &mut booted, &held, "held");
        std::thread::sleep(Duration::from_millis(10));
    }
    pull(&mut painter, 75.0, PointerPhase::Up, 250.0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while painter.is_animating() {
        let posted = painter.post_frame().unwrap();
        loop {
            assert!(Instant::now() < deadline, "the release never came to rest");
            let _ = view.pump();
            if painter.frame_serviced(&posted) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        painter.draw_posted(posted, false).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(75, 2, RED), (75, 48, RED), (75, 55, YELLOW)],
        "released on the header",
    );

    pull(&mut painter, 225.0, PointerPhase::Down, 100.0);
    pull(&mut painter, 225.0, PointerPhase::Move, 250.0);
    pull(&mut painter, 225.0, PointerPhase::Up, 250.0);
    settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(225, 5, YELLOW), (225, 170, GREEN), (75, 2, RED)],
        "no header to pull",
    );
}

const COORDINATOR_WIDTH: u16 = 300;

fn settle<F: bobcat_core::resource::ResourceFetcher + 'static>(
    view: &mut bobcat_core::LynxView<F>,
    painter: &mut bobcat_core::Painter,
    booted: &mut bool,
    expected: &[(u16, u16, [u8; 4])],
    stage: &str,
) -> [u8; 4] {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => *booted = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("[{level}] {message}");
                }
                other => panic!("{stage}: {other:?}"),
            }
        }
        painter.pump().unwrap();
        if *booted {
            let shot = painter.capture().unwrap();
            let pixel = |x: u16, y: u16| {
                let at = (usize::from(y) * usize::from(COORDINATOR_WIDTH) + usize::from(x)) * 4;
                [
                    shot.pixels[at],
                    shot.pixels[at + 1],
                    shot.pixels[at + 2],
                    shot.pixels[at + 3],
                ]
            };
            let seen: Vec<_> = expected.iter().map(|(x, y, _)| pixel(*x, *y)).collect();
            if expected
                .iter()
                .zip(&seen)
                .all(|((_, _, colour), seen)| colour == seen)
            {
                return pixel(150, 30);
            }
            assert!(
                Instant::now() < deadline,
                "{stage}: expected {expected:?}, got {seen:?}"
            );
        } else {
            assert!(Instant::now() < deadline, "BTS boot did not finish");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

async fn paint_and_tap(
    bytes: &[u8],
    point: [u16; 2],
    colors: &[[u8; 4]],
    verification: Option<&str>,
) {
    let page =
        PageSource::from_bytes(&Url::parse("app:///tap.web.bundle").unwrap(), bytes).unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    paint_registered(
        page.view_sources(SCREEN, "en-US"),
        resources,
        point,
        colors,
        verification,
    )
    .await;
}

async fn paint_and_sample(bytes: &[u8], expected: &[(u16, u16, [u8; 4])]) {
    use bobcat_core::{DrawTarget, Painter};
    let page =
        PageSource::from_bytes(&Url::parse("app:///sample.web.bundle").unwrap(), bytes).unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(
            240.0,
            240.0,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN, "en-US"),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    let mut booted = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("[{level}] {message}");
                }
                other => panic!("{other:?}"),
            }
        }
        painter.pump().unwrap();
        if booted {
            let shot = painter.capture().unwrap();
            let seen: Vec<_> = expected
                .iter()
                .map(|(x, y, _)| {
                    let at = (usize::from(*y) * 240 + usize::from(*x)) * 4;
                    [
                        shot.pixels[at],
                        shot.pixels[at + 1],
                        shot.pixels[at + 2],
                        shot.pixels[at + 3],
                    ]
                })
                .collect();
            if expected
                .iter()
                .zip(&seen)
                .all(|((_, _, colour), seen)| colour == seen)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {expected:?}, got {seen:?}"
            );
        } else {
            assert!(Instant::now() < deadline, "BTS boot did not finish");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn paint_registered(
    sources: bobcat_core::ViewSources,
    resources: Resources,
    point: [u16; 2],
    colors: &[[u8; 4]],
    verification: Option<&str>,
) {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(240.0, 240.0, 1.0, resources.builder(), Vec::new(), sources)
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    let mut booted = false;
    let mut verified = verification.is_none();
    for (round, expected) in colors.iter().enumerate() {
        if round > 0 {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                painter.dispatch_input(InputEvent::pointer(
                    Point2D::new(f32::from(point[0]), f32::from(point[1])),
                    1,
                    PointerKind::Touch,
                    phase,
                ));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for event in view.pump() {
                match event {
                    EngineEvent::ScriptFinished => booted = true,
                    EngineEvent::ConsoleMessage { level, message, .. } => {
                        verified |= verification.is_some_and(|required| message.contains(required));
                        eprintln!("[{level}] {message}");
                    }
                    other => panic!("tap round {round}: {other:?}"),
                }
            }
            painter.pump().unwrap();
            if !booted {
                assert!(Instant::now() < deadline, "BTS boot did not finish");
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            let shot = painter.capture().unwrap();
            let offset = (usize::from(point[1]) * 240 + usize::from(point[0])) * 4;
            if booted && verified && shot.pixels[offset..offset + 4] == *expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tap round {round}: expected {expected:?}, got {:?}",
                &shot.pixels[offset..offset + 4]
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
