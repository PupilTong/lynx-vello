//! Every card in `packages/web-core-e2e-fixtures`, rendered and pinned.
//!
//! The corpus is `lynx-stack`'s web-core end-to-end suite, vendored as
//! `ReactLynx` source and compiled for the engine version this repository
//! targets. Upstream checks those cards with Playwright — a screenshot for
//! some, DOM assertions for the rest — against web-core. This suite checks
//! them against *this* engine, and a golden here is only ever a rendering
//! someone read the case for and judged right.
//!
//! **Comparing with web-core's screenshots directly would not say that.** The
//! two stacks rasterize text differently, upstream captures at its own
//! viewport, and several cases are about behaviour an image only indirectly
//! shows. What the case tests has to be read, and our answer judged, before a
//! picture of it is worth keeping.
//!
//! So each card sits in exactly one list:
//!
//! * [`verified!`] — the case was read, this engine's rendering of it is right, and the golden
//!   beside this file is that rendering. It fails when the rendering changes, which is then either
//!   a regression or a new judgement to make.
//! * [`pending!`] — nobody has judged it yet, or the rendering is wrong and the reason says what
//!   about it. There is deliberately no golden: an image of a wrong rendering is worse than none,
//!   because the next reader would take it for a decision. `no_pending_case_has_a_golden` holds
//!   that line.
//!
//! `unlisted_cases_are_an_error` fails when the corpus gains a card that is in
//! neither list, so the suite cannot quietly stop covering the corpus.
//!
//! **A first screen is not the whole of most cases.** Upstream clicks, calls
//! the host element, turns the wheel, and asserts after each. [`driven!`]
//! holds, per card, what upstream drives, as [`Step`]s an embedder of this
//! engine can take — an input event at a viewport point, a call on the view —
//! and the suite pins one more golden per [`Frame`] of that script. A script
//! is not a verdict: a pending card may have one, and then has no golden for
//! any frame of it.
//!
//! **A card is booted the way upstream's shell boots it**
//! (`web-core-e2e/shell-project/index.ts`): the same page data, the same two
//! native modules, the injected sheet and the screen the one card each is for,
//! and upstream's `/dist/` and `/resources/` where the cards reach for them.
//! Each of those was found by a card that rendered wrong for no reason of the
//! engine's, so a card that looks wrong is first checked against that file.
//!
//! Accepting a rendering is `FLASHBULB_UPDATE_SNAPSHOTS=1` on that one test,
//! *after* reading the case and deciding — and moving it to [`verified!`] in
//! the same change, or `no_pending_case_has_a_golden` fails.
//!
//! Requires the corpus: `pnpm --filter web-core-e2e-fixtures build`. Cards
//! fetch their own bitmaps and fonts through `file:` URLs the build bakes in,
//! so nothing has to be served for them to resolve.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::input::{InputEvent, PointerKind, PointerPhase};
use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, ModuleCall, NativeModule, NoWakeup,
    Painter, PreparsedDeclaration, PreparsedRule, PreparsedStyleSheet, ScreenMetrics, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::{LazyBundleInstaller, PageSource};
use flashbulb::{Image, Screenshots};
use url::Url;

/// The viewport upstream's Playwright project uses, and the one every case was
/// written against.
const VIEWPORT: (f32, f32) = (393.0, 727.0);
/// Time a card is given to boot its background thread.
const BOOT_DEADLINE: Duration = Duration::from_secs(30);
/// Time to keep pumping after boot, so a `useEffect` patch, a lazy child and
/// an image read all land before the capture.
const SETTLE: Duration = Duration::from_millis(500);
/// Cards that reach the state their case is about from a timer of their own
/// that is as long as [`SETTLE`]. Both wait 500 ms after mounting, and
/// upstream waits a second for them; a first screen taken after one settle
/// could be either side of that timer, so these get a second one.
const LATE: &[&str] = &[
    "api-boundingclientrect-lynx-view-relative",
    "api-getJSModule",
];
/// How long a card may go on changing before the suite refuses to pin it.
const STABLE_DEADLINE: Duration = Duration::from_secs(10);
/// Turns between two capture attempts, each a 5 ms sleep apart.
const STABLE_TICKS: usize = 20;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> sits two levels under the repository")
        .to_path_buf()
}

fn corpus() -> PathBuf {
    repository().join("packages/web-core-e2e-fixtures/dist")
}

/// Mounts the vendored bitmaps and fonts where a card's own runtime-relative
/// `src` resolves to them.
///
/// Most cards `import` their asset, and the build bakes an absolute `file:`
/// URL for it. A few instead write the path as a plain string —
/// `src='../../../resources/lynx-logo.jpeg'` — which the engine resolves
/// against the template's URL at runtime, so no build step can see it.
/// Upstream serves the package root, with the card's bundle at `/<case>.web.bundle`,
/// so that string climbs past the root and lands on `/resources/lynx-logo.jpeg`.
/// Registering the same four files under `app:///resources/` reproduces that
/// exactly, and a card naming an asset that upstream does not serve either —
/// `basic-performance-image-100` asks for `placeholder.png` — still finds
/// nothing, which is the behaviour under test.
fn mount_resources(resources: &Resources) {
    let directory = repository().join("packages/web-core-e2e-fixtures/resources");
    for entry in std::fs::read_dir(&directory)
        .expect("the vendored resources")
        .flatten()
    {
        let name = entry.file_name();
        let name = name.to_str().expect("an ASCII resource name");
        let bytes = std::fs::read(entry.path()).expect("read the resource");
        resources
            .register(&format!("app:///resources/{name}"), bytes, None)
            .expect("register the resource");
    }
}

/// Mounts the bundles a card names by a rooted `/dist/…` path.
///
/// Upstream serves its whole build output at `/dist/`, and some cards reach
/// into it at runtime: a lazy card imports
/// `/dist/config-lazy-component-….web.bundle`, a frame card sets
/// `src='/dist/api-frame-inner.web.bundle'`. Against the card's own URL those
/// are `app:///dist/…`. The lazy containers are built into `dist/containers/`
/// here, and `api-frame-inner` is a card of the corpus; both go where
/// upstream has them. `basic-lazy-component-fail` asks for
/// `/dist/nonexistent.web.bundle`, which stays missing on purpose.
fn mount_bundles(resources: &Resources) {
    let dist = corpus();
    let mut bundles = vec![dist.join("api-frame-inner.web.bundle")];
    bundles.extend(
        std::fs::read_dir(dist.join("containers"))
            .expect("the built lazy containers")
            .flatten()
            .map(|entry| entry.path()),
    );
    for path in bundles {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("an ASCII bundle name");
        let bytes = std::fs::read(&path).expect("read the bundle");
        resources
            .register(&format!("app:///dist/{name}"), bytes, None)
            .expect("register the bundle");
    }
}

/// Initial page data, as upstream's shell hands it to every card.
const INIT_DATA: &str = r#"{"mockData":"mockData"}"#;
/// Initial global properties, likewise.
const GLOBAL_PROPS: &str = r#"{"backgroundColor":"pink"}"#;

/// `.injected-style-rules { background: green }`, the single rule upstream's
/// shell injects for `api-inject-style-rules`.
fn injected_style_rules() -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![PreparsedRule::Style {
            selectors: ".injected-style-rules".to_owned(),
            declarations: vec![PreparsedDeclaration {
                property: "background".to_owned(),
                value: "green".to_owned(),
                important: false,
            }],
        }],
    }
}

/// `NativeModules.CustomModule.getColor(data, callback)` and
/// `NativeModules.bridge.call('getColor', data, callback)`, answered the way
/// upstream's shell answers both: the callback gets `data.color`.
///
/// Web core routes the two through one host hook, `onNativeModulesCall`,
/// with `bridge` built in. Here a module is whatever the embedder registers
/// under a name, so the shell's hook is two of them.
struct ColorModule {
    name: &'static str,
    method: &'static str,
    /// How long the answer is withheld. Upstream's shell installs its hook
    /// 2.5 s late for `api-nativemodules-call-delay`, so the card's call
    /// waits that long for a reply; an embedder module that answers that
    /// much later is the same sequence seen from the card.
    answers_after: Duration,
}

impl NativeModule for ColorModule {
    fn name(&self) -> &str {
        self.name
    }

    fn methods(&self) -> Vec<String> {
        vec![self.method.to_owned()]
    }

    fn invoke(&self, mut call: ModuleCall) {
        let arguments: serde_json::Value =
            serde_json::from_str(&call.arguments).expect("a module call's arguments are JSON");
        let arguments = arguments.as_array().map_or(&[][..], Vec::as_slice);
        // `bridge.call` names the method in its first argument, and the
        // shell answers `getColor` alone.
        if self.method == "call"
            && arguments.first().and_then(|name| name.as_str()) != Some("getColor")
        {
            return;
        }
        let color = arguments.iter().find_map(|argument| argument.get("color"));
        let (Some(color), Some(callback)) = (color, call.callbacks.pop()) else {
            return;
        };
        let answer = serde_json::Value::Array(vec![color.clone()]).to_string();
        let delay = self.answers_after;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            callback.invoke(answer);
        });
    }
}

/// The native modules upstream's shell gives every card.
fn shell_modules(case: &str) -> Vec<Box<dyn NativeModule>> {
    let answers_after = if case == "api-nativemodules-call-delay" {
        Duration::from_millis(2500)
    } else {
        Duration::ZERO
    };
    vec![
        Box::new(ColorModule {
            name: "CustomModule",
            method: "getColor",
            answers_after,
        }),
        Box::new(ColorModule {
            name: "bridge",
            method: "call",
            answers_after,
        }),
    ]
}

/// The screen a card is told it is on.
///
/// The viewport's own, except for the one card upstream's shell hands a
/// `browserConfig` to: `api-createLynxView-browserConfig` prints
/// `SystemInfo.pixelWidth` and `pixelHeight` and expects the configured
/// 1234 and 5678, which here is the screen the host names in
/// `ViewSources::screen`.
fn screen(case: &str) -> ScreenMetrics {
    let (width, height) = VIEWPORT;
    if case == "api-createLynxView-browserConfig" {
        ScreenMetrics {
            pixel_ratio: 1.0,
            pixel_width: 1234.0,
            pixel_height: 5678.0,
        }
    } else {
        ScreenMetrics::for_viewport(width, height, 1.0)
    }
}

fn screenshots() -> Screenshots {
    // A budget, not a tolerance for being wrong: two runs of the same card on
    // the same engine differ by a handful of glyph pixels — 4 on
    // `config-css-inheritance-true`, 11 on `basic-performance-text-200`, each
    // an isolated pixel inside a text row — because the rasterizer is not bit
    // deterministic between runs. Without a budget those cards could not hold
    // a golden at all; with one this size, a real change still fails, since
    // the smallest of them moves whole glyphs or boxes. The nondeterminism
    // itself is worth fixing in the engine; until it is, this is what lets the
    // suite mean something.
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
        .with_options(flashbulb::CompareOptions::default().with_max_diff_pixels(64))
}

fn golden_path(name: &str) -> PathBuf {
    screenshots().path(&["web-core-e2e", name])
}

/// One thing the suite does to a running card, between two frames.
///
/// Each is something upstream's Playwright spec does to the page, restated in
/// the only terms an embedder of this engine has: an input event at a point
/// of the viewport, or a call on the view. There is no locator — upstream's
/// `page.locator('#target').click()` lands on the centre of that element's
/// box, and the script here says where that is.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// A finger put down and lifted at this viewport point, which is what the
    /// engine synthesizes `tap` from.
    Tap(f32, f32),
    /// A finger put down at the first viewport point, dragged to the second
    /// and lifted there: upstream's `swipe`.
    Drag((f32, f32), (f32, f32)),
    /// A wheel turn over this viewport point, by this many CSS pixels on each
    /// axis: upstream's `locator.hover()` followed by `mouse.wheel(dx, dy)`.
    Wheel((f32, f32), (f32, f32)),
    /// The host's `updateData`, with the default processor: JSON text.
    UpdateData(&'static str),
    /// The host's `updateGlobalProps`: JSON text.
    UpdateGlobalProps(&'static str),
    /// The host's `reload`. Upstream calls it with no argument, which reloads
    /// with the data the view already has; here the host names that data, so
    /// it is [`INIT_DATA`] again.
    Reload,
    /// The host's `sendGlobalEvent`: the event's name, then its argument list
    /// as JSON text.
    SendGlobalEvent(&'static str, &'static str),
    /// Lets this many milliseconds pass. The settle before every capture
    /// already covers a spec's short `wait`; this is for a card that does
    /// something on a timer of its own, later than that.
    Wait(u64),
    /// Waits until the card has logged this exact line, and fails the frame
    /// if it never does. Several cases are about a callback running at all,
    /// and their only evidence is a `console` line — a frame captured after
    /// one says the picture is what the card looks like *once that happened*.
    Logged(&'static str),
}

/// A frame the suite pins after a card's first screen: what is done to the
/// card to reach it, and the name its golden carries.
struct Frame {
    /// The golden is `<case>.<label>.png`.
    label: &'static str,
    steps: &'static [Step],
}

/// How long a [`Step::Logged`] line may take to arrive.
const LOGGED_DEADLINE: Duration = Duration::from_secs(5);

/// One booted card, kept running so it can be driven past its first screen.
///
/// Field order is drop order: the painter lets go of the view before the view
/// ends, and the view ends before its group does.
struct Card {
    case: String,
    painter: Painter,
    view: LynxView<ViewResources>,
    _group: LynxGroup,
    _resources: Resources,
    /// Every `console` line the card has written, oldest first.
    logged: Vec<String>,
    /// How much of [`Self::logged`] a [`Step::Logged`] has already matched,
    /// so a line a card writes once per tap is waited for once per tap.
    heard: usize,
    /// Everything else the view reported, for the reader of a dump.
    events: Vec<String>,
}

impl Card {
    /// Boots a card and lets its first screen settle.
    async fn boot(case: &str) -> Self {
        let path = corpus().join(format!("{case}.web.bundle"));
        let bytes = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error}\nrun `pnpm --filter web-core-e2e-fixtures build` first",
                path.display()
            )
        });
        let input = Url::parse(&format!("app:///{case}.web.bundle")).unwrap();
        let page = PageSource::from_bytes(&input, &bytes).expect("decode the compiled card");
        // The fetcher resolves against the view's own base, which is the
        // card's URL — the contract `ResourcesConfig::base_url` states.
        // Without it a card that writes a relative `src` as a plain string
        // gets `relative URL without a base` and paints nothing, which looks
        // exactly like a bitmap that failed to decode.
        let resources = Resources::new(
            ResourcesConfig {
                base_url: Some(input.clone()),
                // Without an installer every `lynx.fetchBundle` fails, and a
                // lazy card shows its fallback for ever — the picture of a
                // load that failed, for a card about one that succeeds.
                container_installer: Some(Arc::new(LazyBundleInstaller)),
                ..ResourcesConfig::default()
            },
            || {},
        );
        page.register_with(&resources);
        mount_resources(&resources);
        mount_bundles(&resources);

        let (width, height) = VIEWPORT;
        let mut sources = page.view_sources(screen(case));
        // The page data every card was written against. Upstream's shell sets
        // these on the host element before it loads any card
        // (`shell-project/index.ts`), so `useInitData().mockData` is
        // `'mockData'` and `lynx.__globalProps.backgroundColor` is `'pink'`
        // throughout the corpus. Leaving them unset does not make a card test
        // less — it makes it test nothing: `api-initdata` paints its "no
        // data" branch and passes a golden of the wrong input.
        sources.init_data = Some(INIT_DATA.to_owned());
        sources.global_props = Some(GLOBAL_PROPS.to_owned());
        if case == "api-inject-style-rules" {
            // The one card upstream's shell gives an extra author sheet to.
            // Web core takes it as `injectStyleRules`, a list of rule texts;
            // here the same thing is an author stylesheet the host registers
            // and names in `ViewSources::style_sheets`, which is the engine's
            // whole surface for it.
            let url = resources
                .register_style_sheet("app:///injected.css", injected_style_rules())
                .expect("register the injected sheet");
            sources.style_sheets.push(url.to_string());
        }
        let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
            .await
            .unwrap();
        let view = group
            .create_lynx_view(
                width,
                height,
                1.0,
                resources.builder(),
                shell_modules(case),
                sources,
            )
            .unwrap();
        let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
            .await
            .unwrap();
        painter.attach(&view).unwrap();

        let mut card = Self {
            case: case.to_owned(),
            painter,
            view,
            _group: group,
            _resources: resources,
            logged: Vec::new(),
            heard: 0,
            events: Vec::new(),
        };
        let deadline = Instant::now() + BOOT_DEADLINE;
        while !card.turn() {
            assert!(Instant::now() < deadline, "{case}: the card never booted");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        card.settle().await;
        if LATE.contains(&case) {
            card.settle().await;
        }
        card
    }

    /// One turn of the view and of the painter. Says whether the view
    /// reported that it finished booting during it.
    fn turn(&mut self) -> bool {
        let mut booted = false;
        for event in self.view.pump() {
            match event {
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::ConsoleMessage {
                    level,
                    message,
                    source,
                } => {
                    eprintln!("[{level}] [{source:?}] {message}");
                    self.logged.push(message);
                }
                // A card that reports an error still has a screen, and whether
                // that screen is right is what the reader judged, so nothing
                // here fails the test on its own.
                other => {
                    eprintln!("[event] {other:?}");
                    self.events.push(format!("{other:?}"));
                }
            }
        }
        // `tick`, not `pump`: an offscreen painter's `draw` returns at once
        // because there is no window to present to, so a `pump` settle never
        // publishes a clock and every card's animation timeline stays pinned
        // at zero. `tick` is the offscreen turn — it reads the clock, services
        // the gestures and composes — so a card whose screen depends on time
        // (a CSS animation, a transition, a scroll-driven sample) is a thing
        // this suite can judge at all.
        self.painter.tick(false).unwrap();
        booted
    }

    /// Keeps turning for [`SETTLE`], so a `useEffect` patch, a lazy child, an
    /// image read and the reply to an event all land before a capture.
    async fn settle(&mut self) {
        let until = Instant::now() + SETTLE;
        while Instant::now() < until {
            self.turn();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Captures the frame the card has stopped changing on.
    ///
    /// A fixed settle is not enough on its own: a font that resolves or an
    /// image that decodes after it leaves the capture showing one state on
    /// one run and another on the next, which in a golden suite is a test
    /// that fails for no reason anybody can read. So captures are taken until
    /// two in a row are identical, and a card that never stops changing says
    /// so rather than pinning whichever frame it happened to be on.
    async fn frame(&mut self) -> Result<Image, String> {
        let stable_by = Instant::now() + STABLE_DEADLINE;
        let mut previous: Option<Vec<u8>> = None;
        loop {
            let shot = self.painter.capture().expect("capture the screen");
            if previous.as_deref() == Some(shot.pixels.as_slice()) {
                return Ok(
                    Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
                        .expect("captured RGBA image"),
                );
            }
            if Instant::now() >= stable_by {
                return Err(
                    "the screen never stopped changing, so no golden of it would mean anything"
                        .to_owned(),
                );
            }
            previous = Some(shot.pixels);
            for _ in 0..STABLE_TICKS {
                self.turn();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    /// Does one thing to the card. An `Err` says what the card did not do.
    async fn apply(&mut self, step: Step) -> Result<(), String> {
        let refused = |call: &str, error: EngineError| format!("{call} was refused: {error}");
        match step {
            Step::Tap(x, y) => {
                for phase in [PointerPhase::Down, PointerPhase::Up] {
                    self.painter.dispatch_input(InputEvent::pointer(
                        (x, y),
                        1,
                        PointerKind::Touch,
                        phase,
                    ));
                    self.turn();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Step::Drag(from, to) => {
                // Four moves between the two ends, so the sequence travels
                // the way a finger does rather than jumping.
                const MOVES: u8 = 4;
                let along = |step: u8| {
                    let t = f32::from(step) / f32::from(MOVES);
                    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
                };
                let mut sequence = vec![(PointerPhase::Down, from)];
                sequence.extend((1..=MOVES).map(|step| (PointerPhase::Move, along(step))));
                sequence.push((PointerPhase::Up, to));
                for (phase, at) in sequence {
                    self.painter.dispatch_input(InputEvent::pointer(
                        at,
                        1,
                        PointerKind::Touch,
                        phase,
                    ));
                    self.turn();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Step::Wheel(at, delta) => {
                self.painter.dispatch_input(InputEvent::wheel(at, delta));
                self.turn();
            }
            Step::UpdateData(data) => self
                .view
                .update_data(data.to_owned(), String::new())
                .map_err(|error| refused("updateData", error))?,
            Step::UpdateGlobalProps(data) => self
                .view
                .update_global_props(data.to_owned())
                .map_err(|error| refused("updateGlobalProps", error))?,
            Step::Reload => self
                .view
                .reload(INIT_DATA.to_owned(), String::new())
                .map_err(|error| refused("reload", error))?,
            Step::SendGlobalEvent(name, arguments) => self
                .view
                .send_global_event(name, arguments.to_owned())
                .map_err(|error| refused("sendGlobalEvent", error))?,
            Step::Wait(milliseconds) => {
                let until = Instant::now() + Duration::from_millis(milliseconds);
                while Instant::now() < until {
                    self.turn();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Step::Logged(line) => {
                let deadline = Instant::now() + LOGGED_DEADLINE;
                loop {
                    let unheard = &self.logged[self.heard..];
                    if let Some(at) = unheard.iter().position(|logged| logged == line) {
                        self.heard += at + 1;
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Err(format!("the card never logged {line:?}"));
                    }
                    self.turn();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
        }
        Ok(())
    }
}

/// The golden name of a card's first screen is the card's own; a later frame
/// appends its label.
fn golden_name(case: &str, label: Option<&str>) -> String {
    label.map_or_else(|| case.to_owned(), |label| format!("{case}.{label}"))
}

/// The frames a card's script pins after its first screen.
fn script(case: &str) -> &'static [Frame] {
    DRIVEN
        .iter()
        .find(|(driven, _)| *driven == case)
        .map_or(&[], |(_, frames)| frames)
}

/// Every frame of a card, in order, each under its golden name: the first
/// screen, then one per [`Frame`] of its script. Stops at the first step the
/// card does not answer, and says which.
async fn frames(card: &mut Card) -> (Vec<(String, Image)>, Result<(), String>) {
    let case = card.case.clone();
    let mut taken = Vec::new();
    let mut label = None;
    let mut steps: &[Step] = &[];
    let mut rest = script(&case).iter();
    loop {
        let name = golden_name(&case, label);
        for step in steps {
            if let Err(error) = card.apply(*step).await {
                return (taken, Err(format!("{name}: {step:?}: {error}")));
            }
        }
        if label.is_some() {
            card.settle().await;
        }
        match card.frame().await {
            Ok(image) => taken.push((name, image)),
            Err(error) => return (taken, Err(format!("{name}: {error}"))),
        }
        let Some(next) = rest.next() else {
            return (taken, Ok(()));
        };
        label = Some(next.label);
        steps = next.steps;
    }
}

/// Renders a card, drives it through its script, and holds every frame to the
/// rendering that was judged right.
async fn renders_as_judged(case: &str) {
    let mut card = Card::boot(case).await;
    let (taken, outcome) = frames(&mut card).await;
    if let Err(error) = outcome {
        panic!("{case}: {error}");
    }
    for (name, actual) in &taken {
        assert!(
            golden_path(name).exists()
                || std::env::var(flashbulb::UPDATE_ENV).as_deref() == Ok("1"),
            "{name}: no golden. A verified case keeps the rendering someone judged \
             right; accept one with {}=1 only after reading the case",
            flashbulb::UPDATE_ENV,
        );
        screenshots().assert_matches(&["web-core-e2e", name], actual);
    }
}

macro_rules! verified {
    ($($name:ident => $case:literal,)*) => {
        /// Cards whose rendering has been read against the case and judged right.
        const VERIFIED: &[&str] = &[$($case,)*];
        $(
            #[tokio::test]
            async fn $name() {
                renders_as_judged($case).await;
            }
        )*
    };
}

macro_rules! pending {
    ($($case:literal => $reason:literal,)*) => {
        /// Cards nobody has judged yet, each with what stands in the way.
        const PENDING: &[(&str, &str)] = &[$(($case, $reason),)*];
    };
}

macro_rules! driven {
    ($($case:literal => [$($label:literal: [$($step:expr),* $(,)?]),* $(,)?],)*) => {
        /// Cards with something to do after the first screen, and the frames
        /// that pins. A script says what upstream's spec drives; whether the
        /// frames it reaches are right is for `verified!` and `pending!` to say.
        const DRIVEN: &[(&str, &[Frame])] = &[
            $(($case, &[$(Frame { label: $label, steps: &[$($step),*] }),*]),)*
        ];
    };
}

include!("web_core_e2e/verified.rs");
include!("web_core_e2e/pending.rs");
include!("web_core_e2e/driven.rs");

/// The golden directory holds the frames of verified cards and nothing else.
///
/// A pending case must have no golden — neither of its first screen nor of
/// any frame its script reaches: a picture of a rendering nobody has judged
/// would be read as a decision by whoever comes next. The same goes for a
/// picture no card owns any more.
#[test]
fn no_pending_case_has_a_golden() {
    let judged: BTreeSet<String> = VERIFIED
        .iter()
        .flat_map(|case| {
            std::iter::once(golden_name(case, None)).chain(
                script(case)
                    .iter()
                    .map(|frame| golden_name(case, Some(frame.label))),
            )
        })
        .collect();
    let directory = golden_path("any");
    let directory = directory.parent().expect("the golden directory");
    let mut strays: Vec<String> = std::fs::read_dir(directory)
        .expect("the golden directory")
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".png"))
                .map(str::to_owned)
        })
        .filter(|name| !judged.contains(name))
        .collect();
    strays.sort_unstable();
    assert!(
        strays.is_empty(),
        "these goldens belong to no frame of a verified case — move the card to \
         `verified!` if the rendering was judged right, or delete the file: {strays:?}",
    );
}

/// Every card in the corpus is in one of the two lists, nothing names a card
/// that is gone, and every script belongs to a listed card.
#[test]
fn unlisted_cases_are_an_error() {
    let dist = corpus();
    let Ok(entries) = std::fs::read_dir(&dist) else {
        eprintln!(
            "{}: no corpus; run `pnpm --filter web-core-e2e-fixtures build`",
            dist.display()
        );
        return;
    };
    let built: BTreeSet<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".web.bundle"))
                .map(str::to_owned)
        })
        .collect();
    let listed: BTreeSet<String> = VERIFIED
        .iter()
        .copied()
        .chain(PENDING.iter().map(|(case, _)| *case))
        .map(str::to_owned)
        .collect();

    let unlisted: Vec<&String> = built.difference(&listed).collect();
    let gone: Vec<&String> = listed.difference(&built).collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "the corpus and this suite disagree.\n  cards with no entry: {unlisted:?}\n  \
         entries with no card: {gone:?}",
    );

    let mut scripted = BTreeSet::new();
    for (case, frames) in DRIVEN {
        assert!(
            listed.contains(*case),
            "{case}: a script for no listed card"
        );
        assert!(scripted.insert(*case), "{case}: two scripts");
        let labels: BTreeSet<&str> = frames.iter().map(|frame| frame.label).collect();
        assert!(
            !frames.is_empty() && labels.len() == frames.len(),
            "{case}: a script pins at least one frame, each under its own label",
        );
    }
}

/// Writes current renderings somewhere to look at, which is how a batch gets
/// read before anything is accepted. Never writes a golden.
///
/// Every frame of every card goes to `<dir>/<golden name>.png`, and what the
/// card logged and reported to `<dir>/<case>.log` — a case whose evidence is a
/// `console` line is read from there. `WEB_CORE_E2E_ONLY` narrows the run to
/// the cards whose name contains one of its comma-separated parts.
#[tokio::test]
#[ignore = "review aid: WEB_CORE_E2E_DUMP=<dir> cargo test -- --ignored dump_every_rendering"]
async fn dump_every_rendering() {
    let Ok(directory) = std::env::var("WEB_CORE_E2E_DUMP") else {
        panic!("set WEB_CORE_E2E_DUMP to the directory to write into");
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).expect("create the dump directory");
    let only = std::env::var("WEB_CORE_E2E_ONLY").unwrap_or_default();
    let only: Vec<&str> = only.split(',').filter(|part| !part.is_empty()).collect();
    let mut cases: Vec<&str> = VERIFIED.to_vec();
    cases.extend(PENDING.iter().map(|(case, _)| *case));
    cases.retain(|case| only.is_empty() || only.iter().any(|part| case.contains(part)));
    cases.sort_unstable();
    for case in cases {
        let mut card = Card::boot(case).await;
        let (taken, outcome) = frames(&mut card).await;
        for (name, image) in &taken {
            image
                .write_png(directory.join(format!("{name}.png")))
                .expect("write the rendering");
        }
        let mut log = Vec::new();
        if let Err(error) = outcome {
            log.push(format!("STOPPED {error}\n"));
        }
        log.extend(card.logged.iter().map(|line| format!("console {line}\n")));
        log.extend(card.events.iter().map(|event| format!("event {event}\n")));
        std::fs::write(directory.join(format!("{case}.log")), log.concat()).expect("write the log");
    }
}
