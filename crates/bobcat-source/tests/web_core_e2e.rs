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
//! Accepting a rendering is `PRIME_RADIANT_UPDATE_SNAPSHOTS=1` on that one test,
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

use bobcat_core::editing::Command;
use bobcat_core::input::{InputEvent, KeyPhase, Modifiers, PointerKind, PointerPhase};
use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, ModuleCall, NativeModule, NoWakeup,
    Painter, PreparsedDeclaration, PreparsedRule, PreparsedStyleSheet, ScreenMetrics, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::{BundleInstaller, PageSource};
use prime_radiant::{Image, Screenshots};
use url::Url;

const VIEWPORT: (f32, f32) = (393.0, 727.0);
const BOOT_DEADLINE: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_millis(500);
const LATE: &[(&str, u32)] = &[
    ("api-boundingclientrect-lynx-view-relative", 1),
    ("api-getJSModule", 1),
    ("basic-element-text-maxline-with-setData", 2),
];
const RESTLESS: &[&str] = &[
    "basic-element-x-swiper-bindchange",
    "basic-element-x-swiper-bindscrollend",
    "basic-element-x-swiper-bindscrollstart",
];
const STABLE_DEADLINE: Duration = Duration::from_secs(10);
const FRAME_DEADLINE: Duration = Duration::from_secs(10);
const STABLE_TICKS: usize = 20;
const QUICK_STABLE_TICKS: usize = 2;

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

const INIT_DATA: &str = r#"{"mockData":"mockData"}"#;
const GLOBAL_PROPS: &str = r#"{"backgroundColor":"pink"}"#;

fn injected_style_rules() -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![PreparsedRule::Style {
            selectors: ".injected-style-rules".to_owned(),
            declarations: vec![PreparsedDeclaration {
                property: "background".into(),
                value: "green".to_owned(),
                important: false,
            }],
        }],
    }
}

struct ColorModule {
    name: &'static str,
    method: &'static str,
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
    prime_radiant::screenshots_in(env!("CARGO_MANIFEST_DIR"))
        .with_options(prime_radiant::CompareOptions::default().with_max_diff_pixels(64))
}

fn golden_path(name: &str) -> PathBuf {
    screenshots().path(&["web-core-e2e", name])
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Tap(f32, f32),
    Drag((f32, f32), (f32, f32)),
    Wheel((f32, f32), (f32, f32)),
    Hold((f32, f32), (f32, f32)),
    Release(f32, f32),
    QuickCapture,
    Press(&'static str, &'static str, bool),
    KeyDown(&'static str, &'static str),
    KeyUp(&'static str, &'static str),
    MouseOver(f32, f32),
    MouseDown(f32, f32),
    MouseUp(f32, f32),
    MouseDrag((f32, f32), (f32, f32)),
    Fill(&'static str),
    Select(usize, usize),
    UpdateData(&'static str),
    UpdateGlobalProps(&'static str),
    Reload,
    SendGlobalEvent(&'static str, &'static str),
    Fresh,
    Wait(u64),
    Logged(&'static str),
    LoggedStart(&'static str),
    LoggedWith(&'static [&'static str]),
}

struct Frame {
    label: &'static str,
    steps: &'static [Step],
}

const COMPANIONS: &[(&str, &str)] = &[("api-getSharedData", "api-setSharedData")];

const LOGGED_DEADLINE: Duration = Duration::from_secs(5);

struct Card {
    case: String,
    painter: Painter,
    view: LynxView<ViewResources>,
    companion: Option<Box<Card>>,
    group: Option<LynxGroup>,
    _resources: Resources,
    logged: Vec<String>,
    heard: BTreeSet<usize>,
    events: Vec<String>,
    stable_ticks: usize,
}

impl Card {
    async fn boot(case: &str) -> Self {
        let companion = match COMPANIONS.iter().find(|(card, _)| *card == case) {
            Some((_, first)) => Some(Box::new(Box::pin(Self::boot(first)).await)),
            None => None,
        };
        let path = corpus().join(format!("{case}.web.bundle"));
        let bytes = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error}\nrun `pnpm --filter web-core-e2e-fixtures build` first",
                path.display()
            )
        });
        let input = Url::parse(&format!("app:///{case}.web.bundle")).unwrap();
        let page = PageSource::defer(&input, bytes).expect("scan the compiled card");
        let resources = Resources::new(
            ResourcesConfig {
                base_url: Some(input.clone()),
                container_installer: Some(Arc::new(BundleInstaller)),
                ..ResourcesConfig::default()
            },
            || {},
        );
        page.register_in_background(&resources);
        mount_resources(&resources);
        mount_bundles(&resources);

        let (width, height) = VIEWPORT;
        let mut sources = page.view_sources(screen(case), "en-US");
        sources.init_data = Some(INIT_DATA.to_owned());
        sources.global_props = Some(GLOBAL_PROPS.to_owned());
        if case == "api-inject-style-rules" {
            let url = resources
                .register_style_sheet("app:///injected.css", Arc::new(injected_style_rules()))
                .expect("register the injected sheet");
            sources.style_sheets.push(url.to_string());
        }
        let group = match companion {
            Some(_) => None,
            None => Some(
                LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
                    .await
                    .unwrap(),
            ),
        };
        let shared = companion.as_deref().map(Card::group);
        let view = group
            .as_ref()
            .or(shared)
            .expect("a card has a group of its own or its companion's")
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
            companion,
            group,
            _resources: resources,
            logged: Vec::new(),
            heard: BTreeSet::new(),
            events: Vec::new(),
            stable_ticks: STABLE_TICKS,
        };
        let deadline = Instant::now() + BOOT_DEADLINE;
        while !card.turn() {
            assert!(Instant::now() < deadline, "{case}: the card never booted");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        card.settle().await;
        let late = LATE
            .iter()
            .find_map(|(late, settles)| (*late == case).then_some(*settles));
        for _ in 0..late.unwrap_or(0) {
            card.settle().await;
        }
        card
    }

    fn group(&self) -> &LynxGroup {
        match (&self.group, &self.companion) {
            (Some(group), _) => group,
            (None, Some(companion)) => companion.group(),
            (None, None) => unreachable!("a card has a group of its own or its companion's"),
        }
    }

    fn pump(&mut self) -> bool {
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
                other => {
                    eprintln!("[event] {other:?}");
                    self.events.push(format!("{other:?}"));
                }
            }
        }
        booted
    }

    fn turn(&mut self) -> bool {
        let mut booted = self.pump();
        let posted = self.painter.post_frame().unwrap();
        let deadline = Instant::now() + FRAME_DEADLINE;
        while !self.painter.frame_serviced(&posted) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_micros(200));
            booted |= self.pump();
        }
        self.painter.draw_posted(posted, false).unwrap();
        booted
    }

    async fn settle(&mut self) {
        let until = Instant::now() + SETTLE;
        while Instant::now() < until {
            self.turn();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn frame(&mut self) -> Result<Image, String> {
        let stable_by = Instant::now() + STABLE_DEADLINE;
        let mut previous: Option<Vec<u8>> = None;
        loop {
            let shot = self.painter.capture().expect("capture the screen");
            if previous.as_deref() == Some(shot.pixels.as_slice()) {
                self.stable_ticks = STABLE_TICKS;
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
            for _ in 0..self.stable_ticks {
                self.turn();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    async fn apply(&mut self, step: Step) -> Result<(), String> {
        let refused = |call: &str, error: EngineError| format!("{call} was refused: {error}");
        match step {
            Step::Press(key, code, shift) => {
                for phase in [KeyPhase::Down, KeyPhase::Up] {
                    self.key(phase, key, code, shift).await;
                }
            }
            Step::KeyDown(key, code) => self.key(KeyPhase::Down, key, code, false).await,
            Step::KeyUp(key, code) => self.key(KeyPhase::Up, key, code, false).await,
            Step::MouseOver(..) | Step::MouseDown(..) | Step::MouseUp(..) | Step::MouseDrag(..) => {
                self.mouse(step).await;
            }
            Step::Tap(x, y) => {
                for phase in [PointerPhase::Down, PointerPhase::Up] {
                    let event = InputEvent::pointer((x, y), 1, PointerKind::Touch, phase);
                    self.input(event).await;
                }
            }
            Step::Drag(from, to) => {
                let mut sequence = vec![(PointerPhase::Down, from)];
                sequence.extend(Self::along(from, to).map(|at| (PointerPhase::Move, at)));
                sequence.push((PointerPhase::Up, to));
                for (phase, at) in sequence {
                    let event = InputEvent::pointer(at, 1, PointerKind::Touch, phase);
                    self.input(event).await;
                }
            }
            Step::Hold(from, to) => {
                let mut sequence = vec![(PointerPhase::Down, from), (PointerPhase::Move, from)];
                sequence.extend(Self::along(from, to).map(|at| (PointerPhase::Move, at)));
                for (phase, at) in sequence {
                    let event = InputEvent::pointer(at, 1, PointerKind::Touch, phase);
                    self.input(event).await;
                }
            }
            Step::Release(x, y) => {
                let up = InputEvent::pointer((x, y), 1, PointerKind::Touch, PointerPhase::Up);
                self.input(up).await;
            }
            Step::QuickCapture => self.stable_ticks = QUICK_STABLE_TICKS,
            Step::Wheel(at, delta) => {
                self.painter.dispatch_input(InputEvent::wheel(at, delta));
                self.turn();
            }
            Step::Fill(text) => {
                let select_all = Command::SetSelection {
                    range: 0..usize::MAX,
                };
                for command in [select_all, Command::Insert(text.to_owned())] {
                    self.edit(command).map_err(|error| refused("edit", error))?;
                }
            }
            Step::Select(start, end) => self
                .edit(Command::SetSelection { range: start..end })
                .map_err(|error| refused("edit", error))?,
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
            Step::Fresh => {
                let case = self.case.clone();
                *self = Self::boot(&case).await;
            }
            Step::Wait(milliseconds) => {
                let until = Instant::now() + Duration::from_millis(milliseconds);
                while Instant::now() < until {
                    self.turn();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Step::Logged(line) => self.hear(line, |logged| logged == line).await?,
            Step::LoggedStart(start) => {
                self.hear(start, |logged| logged.starts_with(start)).await?;
            }
            Step::LoggedWith(fragments) => {
                let what = fragments.join(" … ");
                self.hear(&what, |logged| fragments.iter().all(|f| logged.contains(f)))
                    .await?;
            }
        }
        Ok(())
    }

    fn along(from: (f32, f32), to: (f32, f32)) -> impl Iterator<Item = (f32, f32)> {
        const MOVES: u8 = 4;
        (1..=MOVES).map(move |step| {
            let t = f32::from(step) / f32::from(MOVES);
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
        })
    }

    async fn input(&mut self, event: InputEvent) {
        self.painter.dispatch_input(event);
        self.turn();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    async fn mouse(&mut self, step: Step) {
        match step {
            Step::MouseOver(x, y) => {
                let over = InputEvent::pointer((x, y), 1, PointerKind::Mouse, PointerPhase::Move)
                    .with_buttons(-1, 0);
                self.input(over).await;
            }
            Step::MouseDown(x, y) => {
                let down = InputEvent::pointer((x, y), 1, PointerKind::Mouse, PointerPhase::Down);
                self.input(down).await;
            }
            Step::MouseDrag(from, to) => {
                let down = InputEvent::pointer(from, 1, PointerKind::Mouse, PointerPhase::Down);
                self.input(down).await;
                for at in Self::along(from, to) {
                    let held = InputEvent::pointer(at, 1, PointerKind::Mouse, PointerPhase::Move)
                        .with_buttons(-1, 1);
                    self.input(held).await;
                }
                let up = InputEvent::pointer(to, 1, PointerKind::Mouse, PointerPhase::Up);
                self.input(up).await;
            }
            Step::MouseUp(x, y) => {
                let up = InputEvent::pointer((x, y), 1, PointerKind::Mouse, PointerPhase::Up);
                self.input(up).await;
            }
            _ => unreachable!("not a mouse step: {step:?}"),
        }
    }

    fn edit(&mut self, command: Command) -> Result<(), EngineError> {
        self.view.edit(command)?;
        self.turn();
        Ok(())
    }

    async fn key(&mut self, phase: KeyPhase, key: &str, code: &str, shift: bool) {
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        self.input(InputEvent::key(phase, key, code, false, modifiers))
            .await;
    }

    async fn hear(&mut self, what: &str, matches: impl Fn(&str) -> bool) -> Result<(), String> {
        let deadline = Instant::now() + LOGGED_DEADLINE;
        loop {
            let found = (0..self.logged.len())
                .find(|at| !self.heard.contains(at) && matches(&self.logged[*at]));
            if let Some(at) = found {
                self.heard.insert(at);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("the card never logged {what:?}"));
            }
            self.turn();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn golden_name(case: &str, label: Option<&str>) -> String {
    label.map_or_else(|| case.to_owned(), |label| format!("{case}.{label}"))
}

fn script(case: &str) -> &'static [Frame] {
    DRIVEN
        .iter()
        .find(|(driven, _)| *driven == case)
        .map_or(&[], |(_, frames)| frames)
}

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
        if label.is_some() || !RESTLESS.contains(&case.as_str()) {
            match card.frame().await {
                Ok(image) => taken.push((name, image)),
                Err(error) => return (taken, Err(format!("{name}: {error}"))),
            }
        }
        let Some(next) = rest.next() else {
            return (taken, Ok(()));
        };
        label = Some(next.label);
        steps = next.steps;
    }
}

async fn renders_as_judged(case: &str) {
    let mut card = Card::boot(case).await;
    let (taken, outcome) = frames(&mut card).await;
    if let Err(error) = outcome {
        panic!("{case}: {error}");
    }
    for (name, actual) in &taken {
        assert!(
            golden_path(name).exists()
                || std::env::var(prime_radiant::UPDATE_ENV).as_deref() == Ok("1"),
            "{name}: no golden. A verified case keeps the rendering someone judged \
             right; accept one with {}=1 only after reading the case",
            prime_radiant::UPDATE_ENV,
        );
        screenshots().assert_matches(&["web-core-e2e", name], actual);
    }
}

macro_rules! verified {
    ($($name:ident => $case:literal,)*) => {
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
        const PENDING: &[(&str, &str)] = &[$(($case, $reason),)*];
    };
}

macro_rules! driven {
    ($($case:literal => [$($label:literal: [$($step:expr),* $(,)?]),* $(,)?],)*) => {
        const DRIVEN: &[(&str, &[Frame])] = &[
            $(($case, &[$(Frame { label: $label, steps: &[$($step),*] }),*]),)*
        ];
    };
}

include!("web_core_e2e/verified.rs");
include!("web_core_e2e/pending.rs");
include!("web_core_e2e/driven.rs");

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
