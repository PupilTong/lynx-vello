//! The Lynx main-thread runtime over its owned `QuickJS` realm.
//!
//! # Who owns the document
//!
//! The realm does, and it says so: the boot module's first statement after its
//! configuration literal is `export const document = new Document(config);`,
//! and that constructor is what
//! builds the document — out of the [`DocumentIngredients`] the view task
//! staged before this realm was opened. No JavaScript in this realm runs
//! ahead of that statement, so every host member that follows has a document
//! to work on.
//!
//! The exported binding holds it for the realm's life. Nothing in the realm
//! releases it: there is no release member, no registry, and no answer a tree
//! member gives without a document. Because `bobcat-internal:host` resolves
//! from any module in the realm a card can reach `createDocument` too, and
//! the one refusal left in this area is what it gets: the ingredients are
//! spent, so a second construction fails that card's boot.
//!
//! Release is the view's task ending. Dropping the `LynxView` closes the
//! command channel, the task returns, and [`MainThreadRuntime`]'s fields drop
//! in declaration order — every field that holds a handle of this realm first,
//! which together are what frees it, and with it the host functions it held and
//! their clones of this realm's [`DocumentSlot`]; then the runtime's own `slot`
//! handle, which is when the `LynxDocument` drops. JavaScript goes first, then
//! the Rust object it named.

use std::cell::{Cell, OnceCell, RefCell, RefMut};
use std::fmt::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use terminus::link::event::{EventDetail, ListenerKey, ListenerKinds, RegistrationChange};
use terminus::link::{MethodCall, MethodError, MethodOutcome, StylePool};
use tokio::sync::watch;

use self::pipelines::{
    CONTAINER_TIMING_ATTRIBUTE, CONTAINER_TIMING_IGNORE_ATTRIBUTE, FlushRequest, RealmTiming,
    TIMING_FLAG_ATTRIBUTE,
};
use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::clock::ClockInstant;
use crate::esm::{
    BTS_MODULE_SPECIFIER, ELEMENT_MODULE_SPECIFIER, Export, FRAME_MODULE_SPECIFIER,
    HOST_MODULE_SPECIFIER, RUNTIME_MODULE_SPECIFIER, TIMER_MODULE_SPECIFIER,
};
use crate::link::{InputEventPayload, ViewNotice, ViewOutbox};
use crate::main::exposure::{Exposure, is_exposure_attribute};
use crate::main::layout_change::{
    LAYOUT_CHANGE_EVENT, LayoutChangeObserver, LayoutEventRegistration, TEXT_LAYOUT_EVENT,
};
use crate::main::record::write_record_field;
use crate::main::tree::{
    Autoplays, ComponentDetail, ComponentEvent, ComponentEvents, Field, ImageOutcome, ImageSources,
    LynxDocument, MAX_FRAME_DEPTH, PageConfig, SELECTION_CHANGE, START_PLAY_EVENT,
    SelectionReports, Videos, apply_image_events, apply_video_reports, frame_depth,
    invoke_base_method, is_svg, is_text, nested_page, new_document, relevance_span,
    report_selection_changes, reset_frame,
};
use crate::performance::{Opener, OpenerId};
use crate::realm::child::{ChildHostModules, ChildHostState, ChildKey};
use crate::realm::policy::context_of;
use crate::realm::{RealmCore, RealmId, RealmNames, open_realm, string_argument};
use crate::script::ScriptError;
use crate::timers::run_due_timers;
use crate::view::{LynxViewError, ScreenMetrics, ScriptSource, StartupSource, Viewport};

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";

const DOCUMENT_LEVEL_EVENTS: [&str; 2] = ["keydown", "keyup"];

const FRAME_CONNECTED_EVENT: &str = "bobcat:frameconnected";
const FRAME_DISCONNECTED_EVENT: &str = "bobcat:framedisconnected";
const APPEAR_EVENT: &str = "uiappear";
const DISAPPEAR_EVENT: &str = "uidisappear";

const EXPOSURE_RECORD_EVENT: &str = "bobcat:exposure";

const SCROLL_EVENT: &str = "scroll";
const SCROLL_END_EVENT: &str = "scrollend";

const CLICK_EVENT: &str = "click";
const DOUBLE_CLICK_EVENT: &str = "dblclick";

const DOUBLE_CLICK_MILLISECONDS: f64 = 500.0;

const LOAD_EVENT: &str = "load";
const ERROR_EVENT: &str = "error";

const TAP_EVENT: &str = "tap";

const DETAIL_POSITION: f64 = 0.0;
const DETAIL_EMPTY: f64 = 2.0;
const DETAIL_EXPOSURE: f64 = 3.0;
const DETAIL_KEY: f64 = 4.0;
const DETAIL_INPUT: f64 = 5.0;
const DETAIL_COMPOSITION: f64 = 6.0;
const DETAIL_FOCUS: f64 = 7.0;
const DETAIL_CLONED: f64 = 8.0;
const DETAIL_DISPATCHED: f64 = 9.0;
const DETAIL_CONTENT_VISIBILITY: f64 = 10.0;

const CONTENT_VISIBILITY_AUTO_STATE_CHANGE_EVENT: &str = "contentvisibilityautostatechange";

const MODIFIER_ALT: u8 = 1;
const MODIFIER_CTRL: u8 = 2;
const MODIFIER_META: u8 = 4;
const MODIFIER_SHIFT: u8 = 8;

type Extras<'a> = &'a [(&'static str, HostArgument<'a>)];

fn push_detail_extras<'a>(
    detail: &'a ComponentDetail,
    extras: &mut SmallVec<[(&'static str, HostArgument<'a>); 7]>,
) {
    let unit = |offset: u32| HostArgument::Number(f64::from(offset));
    let optional = |key, value: &'a Option<String>| {
        value
            .as_deref()
            .map(|value| (key, HostArgument::String(value)))
    };
    match detail {
        ComponentDetail::Id { id } => extras.extend(optional("id", id)),
        ComponentDetail::UrlContent {
            url,
            content,
            content_id,
        } => {
            extras.extend([
                ("url", HostArgument::String(url)),
                ("content", HostArgument::String(content)),
            ]);
            extras.extend(optional("contentId", content_id));
        }
        ComponentDetail::Url { url, content_id } => {
            extras.push(("url", HostArgument::String(url)));
            extras.extend(optional("contentId", content_id));
        }
        ComponentDetail::Value { value } => extras.push(("value", HostArgument::String(value))),
        ComponentDetail::Input {
            value,
            selection,
            is_composing,
        } => {
            let length: usize = value.chars().map(char::len_utf16).sum();
            #[expect(
                clippy::cast_precision_loss,
                reason = "a field's length, far inside an f64's integers"
            )]
            let length = length as f64;
            extras.extend([
                ("value", HostArgument::String(value)),
                ("textLength", HostArgument::Number(length)),
                ("cursor", unit(selection.start)),
                ("isComposing", HostArgument::Boolean(*is_composing)),
                ("selectionStart", unit(selection.start)),
                ("selectionEnd", unit(selection.end)),
            ]);
        }
        ComponentDetail::Selection { selection } => extras.extend([
            ("selectionStart", unit(selection.start)),
            ("selectionEnd", unit(selection.end)),
        ]),
        ComponentDetail::Error { code, message } => extras.extend([
            ("errorCode", HostArgument::Number(f64::from(*code))),
            ("errorMsg", HostArgument::String(message)),
        ]),
        ComponentDetail::Fields(fields) => {
            extras.extend(fields.iter().map(|&(key, field)| {
                let value = match field {
                    Field::Number(number) => HostArgument::Number(number),
                    Field::Bool(flag) => HostArgument::Boolean(flag),
                };
                (key, value)
            }));
        }
        ComponentDetail::TextSelection { range, forward } => {
            let (start, end) = range.as_ref().map_or(
                (HostArgument::Number(-1.0), HostArgument::Number(-1.0)),
                |range| (unit(range.start), unit(range.end)),
            );
            let direction = if *forward { "forward" } else { "backward" };
            extras.extend([
                ("start", start),
                ("end", end),
                ("direction", HostArgument::String(direction)),
            ]);
        }
    }
}

fn text_lines_json(lines: &[terminus::link::TextLineReport]) -> String {
    let mut text = String::with_capacity(2 + lines.len() * 12);
    text.push('[');
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        let _ = write!(text, "{},{},{}", line.start, line.end, line.ellipsis_count);
    }
    text.push(']');
    text
}

fn push_text_layout_extras<'a>(
    line_count: usize,
    lines: &'a str,
    width: f32,
    height: f32,
    extras: &mut SmallVec<[(&'static str, HostArgument<'a>); 7]>,
) {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a paragraph's line count, far inside an f64's integers"
    )]
    let count = line_count as f64;
    extras.extend([
        ("lineCount", HostArgument::Number(count)),
        ("lines", HostArgument::String(lines)),
        ("width", HostArgument::Number(f64::from(width))),
        ("height", HostArgument::Number(f64::from(height))),
    ]);
}

type Arguments<'a> = SmallVec<[HostArgument<'a>; 32]>;

struct Invited {
    on_path: SmallVec<[ListenerKey; 2]>,
    globals: SmallVec<[ListenerKey; 2]>,
}

impl Invited {
    fn read(
        document: &LynxDocument,
        steps: &terminus::link::event::EventSteps,
        name: &str,
    ) -> Self {
        let globals = if DOCUMENT_LEVEL_EVENTS.contains(&name) {
            document.global_registrants(name)
        } else {
            document.global_registrants_on_path(steps, name)
        };
        Self {
            on_path: document.listeners_on_path(steps, name),
            globals,
        }
    }

    fn contains(&self, key: ListenerKey) -> bool {
        self.on_path.contains(&key) || self.globals.contains(&key)
    }
}

fn realm_calls(
    document: &LynxDocument,
    steps: &terminus::link::event::EventSteps,
    name: &str,
    registrants: impl Iterator<Item = ListenerKey>,
) -> (SmallVec<[ListenerKey; 2]>, Invited) {
    let invited = Invited::read(document, steps, name);
    let calls = registrants.filter(|&key| invited.contains(key)).collect();
    (calls, invited)
}

fn encode_call<'a>(
    document: &LynxDocument,
    steps: &terminus::link::event::EventSteps,
    name: &'a str,
    bubbles: bool,
    timestamp: f64,
    detail: CallDetail<'a>,
) -> Arguments<'a> {
    let mut arguments = Arguments::new();
    arguments.push(HostArgument::String(name));
    arguments.push(HostArgument::Boolean(bubbles));
    arguments.push(HostArgument::Number(timestamp));
    encode_path(document, steps, &mut arguments);
    match detail {
        CallDetail::Event { detail, extras } => {
            encode_extras(extras, &mut arguments);
            encode_detail(detail, &mut arguments);
        }
        CallDetail::Cloned(detail) => {
            encode_extras(&[], &mut arguments);
            match detail {
                Some(detail) => {
                    arguments.push(HostArgument::Number(DETAIL_CLONED));
                    arguments.push(detail);
                }
                None => arguments.push(HostArgument::Number(DETAIL_EMPTY)),
            }
        }
        CallDetail::Dispatched => {
            encode_extras(&[], &mut arguments);
            arguments.push(HostArgument::Number(DETAIL_DISPATCHED));
        }
    }
    arguments
}

#[derive(Clone, Copy)]
enum CallDetail<'a> {
    Event {
        detail: &'a EventDetail,
        extras: Extras<'a>,
    },
    Cloned(Option<HostArgument<'a>>),
    Dispatched,
}

struct Dispatch<'a> {
    target: terminus::link::NodeId,
    name: &'a str,
    bubbles: bool,
    composed: bool,
    timestamp: f64,
}

enum DispatchDetail<'a> {
    Routed {
        detail: &'a EventDetail,
        extras: Extras<'a>,
    },
    Synthetic {
        own: ListenerKey,
        detail: Option<HostArgument<'a>>,
    },
}

fn run_dispatch(
    slot: &RefCell<DocumentSlot>,
    realms: &RealmCore,
    event: &Dispatch<'_>,
    detail: &DispatchDetail<'_>,
    mut call: impl FnMut(ListenerKey, &Arguments<'_>) -> bool,
) -> Option<bool> {
    let &Dispatch {
        target,
        name,
        bubbles,
        composed,
        timestamp,
    } = event;
    let lowered;
    let registered = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
        lowered = name.to_ascii_lowercase();
        lowered.as_str()
    } else {
        name
    };
    let (steps, calls, invited) = {
        let slot = slot.borrow();
        let document = slot.document.as_ref().expect(DOCUMENT_EXISTS);
        if document.get(target).is_none()
            || !(document.has_listeners_for(registered) || document.observes_event(name))
        {
            return None;
        }
        let registrants = realms.realm_ids().map(RealmId::listener_key);
        let steps = document.event_steps(target, bubbles, composed);
        let (calls, invited) = realm_calls(document, &steps, registered, registrants);
        (steps, calls, invited)
    };
    let mut calls = (!calls.is_empty()).then_some((calls, invited));

    let none = EventDetail::None;
    let (heard, call_detail, own) = match *detail {
        DispatchDetail::Routed { detail, extras } => {
            (detail, CallDetail::Event { detail, extras }, None)
        }
        DispatchDetail::Synthetic { own, detail } => (&none, CallDetail::Cloned(detail), Some(own)),
    };
    let mut walk = slot
        .borrow()
        .document
        .as_ref()
        .expect(DOCUMENT_EXISTS)
        .event_walk(&steps, name, heard, timestamp);
    let mut called = false;
    loop {
        let outcome = slot.borrow_mut().document_mut().advance(&mut walk);
        let Some(outcome) = outcome else {
            break;
        };
        if !outcome.capture {
            continue;
        }
        let Some((calls, invited)) = calls.take() else {
            continue;
        };
        let (arguments, dispatching) = {
            let slot = slot.borrow();
            let document = slot.document.as_ref().expect(DOCUMENT_EXISTS);
            let encode = |detail| encode_call(document, &steps, name, bubbles, timestamp, detail);
            let dispatching = own
                .filter(|own| calls.contains(own))
                .map(|own| (own, encode(CallDetail::Dispatched)));
            (encode(call_detail), dispatching)
        };
        let mut live: Option<Invited> = Some(invited);
        for &key in &calls {
            let invited = live.get_or_insert_with(|| {
                Invited::read(
                    slot.borrow().document.as_ref().expect(DOCUMENT_EXISTS),
                    &steps,
                    registered,
                )
            });
            if !invited.contains(key) {
                continue;
            }
            let arguments = match &dispatching {
                Some((own, arguments)) if *own == key => arguments,
                _ => &arguments,
            };
            called |= call(key, arguments);
            live = None;
        }
    }
    Some(called)
}

fn realm_path_steps<'a>(
    document: &'a LynxDocument,
    steps: &'a terminus::link::event::EventSteps,
) -> impl Iterator<Item = &'a terminus::link::event::EventStep> + 'a {
    steps.steps().iter().rev().filter(|step| {
        step.capture
            && document
                .get(step.node)
                .is_some_and(|node| !node.is_shadow_root())
            && document.get(step.target).is_some()
    })
}

fn encode_path(
    document: &LynxDocument,
    steps: &terminus::link::event::EventSteps,
    arguments: &mut Arguments<'_>,
) {
    let count_at = arguments.len();
    arguments.push(HostArgument::Number(0.0));
    for step in realm_path_steps(document, steps) {
        arguments.push(HostArgument::Number(packed_node_id(step.node)));
        arguments.push(HostArgument::Number(packed_node_id(step.target)));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a path's steps, far inside an f64's integers"
    )]
    let count = ((arguments.len() - count_at - 1) / 2) as f64;
    arguments[count_at] = HostArgument::Number(count);
}

fn encode_extras<'a>(extras: Extras<'a>, arguments: &mut Arguments<'a>) {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a handful of extras, far inside an f64's integers"
    )]
    arguments.push(HostArgument::Number(extras.len() as f64));
    for &(name, value) in extras {
        arguments.push(HostArgument::String(name));
        arguments.push(value);
    }
}

fn pointer_type(device: terminus::link::input::PointerKind) -> &'static str {
    match device {
        terminus::link::input::PointerKind::Touch => "touch",
        terminus::link::input::PointerKind::Pen => "pen",
        _ => "mouse",
    }
}

fn encode_detail<'a>(detail: &'a EventDetail, arguments: &mut Arguments<'a>) {
    let (position, delta, pointer, points): (_, _, _, &[terminus::link::event::TouchPoint]) =
        match detail {
            EventDetail::Pointer {
                position,
                id,
                device,
                button,
                buttons,
            } => (
                *position,
                None,
                Some((*button, *buttons, *id, pointer_type(*device))),
                &[],
            ),
            EventDetail::Wheel { position, delta } => (*position, Some(*delta), None, &[]),
            EventDetail::Touch { position, points } => (*position, None, None, points),
            EventDetail::Exposure => {
                arguments.push(HostArgument::Number(DETAIL_EXPOSURE));
                return;
            }
            EventDetail::Input {
                input_type,
                data,
                is_composing,
            } => {
                arguments.push(HostArgument::Number(DETAIL_INPUT));
                arguments.push(HostArgument::String(input_type));
                arguments.push(
                    data.as_deref()
                        .map_or(HostArgument::Null, HostArgument::String),
                );
                arguments.push(HostArgument::Boolean(*is_composing));
                return;
            }
            EventDetail::Composition { data } => {
                arguments.push(HostArgument::Number(DETAIL_COMPOSITION));
                arguments.push(HostArgument::String(data));
                return;
            }
            EventDetail::Focus { related_target } => {
                arguments.push(HostArgument::Number(DETAIL_FOCUS));
                arguments.push(related_target.map_or(HostArgument::Null, |node| {
                    HostArgument::Number(packed_node_id(node))
                }));
                return;
            }
            EventDetail::Key {
                key,
                code,
                repeat,
                modifiers,
                text: _,
            } => {
                let mut bits = 0;
                for (held, bit) in [
                    (modifiers.alt, MODIFIER_ALT),
                    (modifiers.ctrl, MODIFIER_CTRL),
                    (modifiers.meta, MODIFIER_META),
                    (modifiers.shift, MODIFIER_SHIFT),
                ] {
                    if held {
                        bits |= bit;
                    }
                }
                arguments.push(HostArgument::Number(DETAIL_KEY));
                arguments.push(HostArgument::String(key));
                arguments.push(HostArgument::String(code));
                arguments.push(HostArgument::Boolean(*repeat));
                arguments.push(HostArgument::Number(f64::from(bits)));
                return;
            }
            EventDetail::ContentVisibility { skipped } => {
                arguments.push(HostArgument::Number(DETAIL_CONTENT_VISIBILITY));
                arguments.push(HostArgument::Boolean(*skipped));
                return;
            }
            _ => {
                arguments.push(HostArgument::Number(DETAIL_EMPTY));
                return;
            }
        };
    let number = |value: Option<f64>| value.map_or(HostArgument::Undefined, HostArgument::Number);
    arguments.push(HostArgument::Number(DETAIL_POSITION));
    arguments.push(HostArgument::Number(f64::from(position.x)));
    arguments.push(HostArgument::Number(f64::from(position.y)));
    arguments.push(number(delta.map(|delta| f64::from(delta.x))));
    arguments.push(number(delta.map(|delta| f64::from(delta.y))));
    arguments.push(number(pointer.map(|(button, ..)| f64::from(button))));
    arguments.push(number(pointer.map(|(_, buttons, ..)| f64::from(buttons))));
    arguments.push(number(pointer.map(|(_, _, id, _)| f64::from(id))));
    arguments.push(pointer.map_or(HostArgument::Undefined, |(.., kind)| {
        HostArgument::String(kind)
    }));
    for point in points {
        arguments.push(HostArgument::Number(f64::from(point.identifier)));
        arguments.push(HostArgument::Number(f64::from(point.position.x)));
        arguments.push(HostArgument::Number(f64::from(point.position.y)));
        arguments.push(HostArgument::Number(f64::from(point.flags)));
    }
}

const INLINE_DECLARATIONS: usize = 16;

#[cfg(feature = "devtools")]
pub(crate) mod devtools;
mod editing;
mod pipelines;
mod style_sheets;

pub(crate) fn card_entry(body: &str) -> String {
    let mut module = String::with_capacity(crate::esm::MTS_CHUNK_PREAMBLE.len() + body.len());
    module.push_str(crate::esm::MTS_CHUNK_PREAMBLE);
    module.push_str(body);
    module
}

#[derive(Debug)]
pub(crate) struct MainThreadError {
    context: &'static str,
    source: ScriptError,
}

impl MainThreadError {
    fn from_engine(context: &'static str, source: ScriptError) -> Self {
        Self { context, source }
    }

    pub(crate) fn into_script_error(mut self) -> ScriptError {
        self.source.message = Arc::from(format!("{}: {}", self.context, self.source.message));
        self.source
    }
}

impl fmt::Display for MainThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

enum Checkpoint<'a> {
    Each,
    Deferred(&'a mut SmallVec<[RealmId; 2]>),
}

impl std::error::Error for MainThreadError {}

const REMOVALS_PER_COLLECTION: u32 = 32;

pub(crate) struct DocumentIngredients {
    pub(crate) viewport: Viewport,
    pub(crate) config: PageConfig,
    pub(crate) text_context: terminus::link::TextContext,
    pub(crate) style_pool: Option<Rc<StylePool>>,
}

#[cfg(test)]
pub(crate) fn bound_metrics(viewport: Viewport) -> watch::Receiver<Option<Viewport>> {
    watch::channel(Some(viewport)).1
}

#[cfg(test)]
impl DocumentIngredients {
    pub(crate) fn for_test(viewport: Viewport, config: PageConfig) -> Self {
        Self {
            viewport,
            config,
            text_context: crate::view::stage_text_context(Vec::new(), None, Vec::new(), "en-US")
                .expect("en-US is a language tag"),
            style_pool: None,
        }
    }
}

pub(crate) struct RealmStartup {
    pub(crate) sheets: Vec<StartupSource>,
    pub(crate) screen: ScreenMetrics,
    pub(crate) entry: String,
    pub(crate) card: Option<String>,
    pub(crate) background_entry: Option<String>,
    pub(crate) initial_processor: String,
    pub(crate) init_data: Option<String>,
    pub(crate) global_props: Option<String>,
    pub(crate) native_modules: String,
}

impl Default for RealmStartup {
    fn default() -> Self {
        Self {
            sheets: Vec::new(),
            screen: ScreenMetrics {
                pixel_ratio: 1.0,
                pixel_width: 0.0,
                pixel_height: 0.0,
            },
            entry: String::new(),
            card: None,
            background_entry: None,
            initial_processor: String::new(),
            init_data: None,
            global_props: None,
            native_modules: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Deliveries(u8);

impl Deliveries {
    pub(crate) const NONE: Self = Self(0);
    pub(crate) const RESIZE: Self = Self(1);
    pub(crate) const INTERSECTION: Self = Self(1 << 1);
    pub(crate) const CONTENT_VISIBILITY: Self = Self(1 << 2);
    pub(crate) const COMPONENT_EVENTS: Self = Self(1 << 3);

    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

struct DocumentSlot {
    ingredients: Option<DocumentIngredients>,
    sheets: Vec<StartupSource>,
    failed_sheet: Option<String>,
    document: Option<LynxDocument>,
    metrics: watch::Receiver<Option<Viewport>>,
    bound: bool,
    held: Option<Arc<terminus::link::CommittedFrame>>,
    component_events: ComponentEvents,
    image_sources: ImageSources,
    videos: Videos,
    autoplays: Autoplays,
    exposure: Exposure,
    layout_change: LayoutChangeObserver,
    text_selection_reports: SelectionReports,
    removals: u32,
    editing_routes: u8,
    observed: FxHashSet<&'static str>,
    scrolling: SmallVec<[terminus::link::NodeId; 4]>,
    timeline_milliseconds: f64,
    pending_flags: Vec<Arc<str>>,
    native_pipelines: Vec<u64>,
    content_detection: bool,
    card_realms: FxHashMap<OpenerId, ChildKey>,
    outbox: ViewOutbox,
}

trait RouteName {
    fn as_str(&self) -> &str;

    fn to_arc(&self) -> Arc<str>;
}

impl RouteName for &str {
    fn as_str(&self) -> &str {
        self
    }

    fn to_arc(&self) -> Arc<str> {
        Arc::from(*self)
    }
}

impl RouteName for Arc<str> {
    fn as_str(&self) -> &str {
        self
    }

    fn to_arc(&self) -> Arc<str> {
        Arc::clone(self)
    }
}

const DOCUMENT_EXISTS: &str = "the boot module creates the document before any card runs";

impl DocumentSlot {
    fn new(
        ingredients: DocumentIngredients,
        sheets: Vec<StartupSource>,
        metrics: watch::Receiver<Option<Viewport>>,
        outbox: ViewOutbox,
    ) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            ingredients: Some(ingredients),
            sheets,
            failed_sheet: None,
            document: None,
            metrics,
            bound: false,
            held: None,
            component_events: ComponentEvents::default(),
            image_sources: ImageSources::default(),
            videos: Videos::default(),
            autoplays: Autoplays::default(),
            exposure: Exposure::new(),
            layout_change: LayoutChangeObserver::default(),
            text_selection_reports: SelectionReports::default(),
            removals: 0,
            editing_routes: 0,
            observed: FxHashSet::default(),
            scrolling: SmallVec::new(),
            timeline_milliseconds: 0.0,
            pending_flags: Vec::new(),
            native_pipelines: Vec::new(),
            content_detection: false,
            card_realms: FxHashMap::default(),
            outbox,
        }))
    }

    fn document_mut(&mut self) -> &mut LynxDocument {
        self.document.as_mut().expect(DOCUMENT_EXISTS)
    }

    fn document_ref(&self) -> &LynxDocument {
        self.document.as_ref().expect(DOCUMENT_EXISTS)
    }

    fn run_due_autoplays(&mut self) {
        let Some(deadline) = self.autoplays.next_deadline() else {
            return;
        };
        let now = ClockInstant::now();
        if deadline > now {
            return;
        }
        let autoplays = self.autoplays.clone();
        autoplays.run_due(self.document_mut(), now);
    }

    fn create_document(&mut self, config: PageConfig) -> Result<(), String> {
        let Some(ingredients) = self.ingredients.take() else {
            return Err("the realm already created its document".to_owned());
        };
        let DocumentIngredients {
            viewport,
            config: _,
            text_context,
            style_pool,
        } = ingredients;
        let viewport = match *self.metrics.borrow_and_update() {
            Some(metrics) => {
                self.bound = true;
                metrics
            }
            None => viewport,
        };
        let events = self.component_events.clone();
        let autoplays = self.autoplays.clone();
        let images = self.image_sources.clone();
        let videos = self.videos.clone();
        let document = construction_phase("building the page", || {
            let mut document = new_document(viewport, config, events, autoplays, images, videos);
            if let Some(pool) = style_pool {
                document.set_style_pool(pool);
            }
            document.adopt_text_context(text_context);
            document
        })?;
        self.document = Some(document);
        Ok(())
    }

    fn instant_scrolls(&mut self) {
        let hears_scroll = self.listens(SCROLL_EVENT);
        let document = self.document.as_mut().expect(DOCUMENT_EXISTS);
        let moved = document.take_instant_scrolls();
        for (node, previous) in moved {
            if hears_scroll {
                let delta = document.scroll_offset(node) - previous;
                self.component_events
                    .queue_scroll(node, SCROLL_EVENT, delta);
            }
            if !self.scrolling.contains(&node) {
                self.scrolling.push(node);
            }
        }
    }

    fn listens(&self, name: &str) -> bool {
        self.registered(name) || self.observed.contains(name)
    }

    fn registered(&self, name: &str) -> bool {
        self.document
            .as_ref()
            .is_some_and(|document| document.has_listeners_for(name))
    }

    fn open_listener(
        &mut self,
        node: terminus::link::NodeId,
        name: &str,
        kinds: ListenerKinds,
        key: ListenerKey,
    ) {
        self.apply_pending_last_removals();
        let exposure = is_exposure_event(name);
        let layout = LayoutEventRegistration::of(name);
        let document = self.document.as_mut().expect(DOCUMENT_EXISTS);
        let before = exposure.then(|| exposure_rows(document, node));
        let layout_before =
            layout.map(|registration| (registration, document.has_listener(node, name)));
        let change = document.add_listener(node, name, kinds, key);
        if let Some(before) = before {
            self.exposure_follows(node, before);
        }
        if let Some((registration, before)) = layout_before {
            self.layout_registration_follows(node, registration, before);
        }
        self.publish_route_changes(&[(name, change)]);
    }

    fn close_listener(&mut self, node: terminus::link::NodeId, name: &str, key: ListenerKey) {
        self.apply_pending_last_removals();
        let exposure = is_exposure_event(name);
        let layout = LayoutEventRegistration::of(name);
        let document = self.document.as_mut().expect(DOCUMENT_EXISTS);
        let before = exposure.then(|| exposure_rows(document, node));
        let layout_before =
            layout.map(|registration| (registration, document.has_listener(node, name)));
        let change = document.remove_listener(node, name, key);
        if let Some(before) = before {
            self.exposure_follows(node, before);
        }
        if let Some((registration, before)) = layout_before {
            self.layout_registration_follows(node, registration, before);
        }
        self.publish_route_changes(&[(name, change)]);
    }

    fn layout_registration_follows(
        &mut self,
        node: terminus::link::NodeId,
        registration: LayoutEventRegistration,
        before: bool,
    ) {
        let Self {
            document,
            layout_change,
            ..
        } = self;
        let document = document.as_mut().expect(DOCUMENT_EXISTS);
        let now = document.has_listener(node, registration.name());
        if now == before
            || !document
                .get(node)
                .is_some_and(terminus::link::Node::is_element)
        {
            return;
        }
        match registration {
            LayoutEventRegistration::ResizeObservation if now => {
                layout_change.observe(document, node);
            }
            LayoutEventRegistration::ResizeObservation => {
                layout_change.unobserve(document, node);
            }
            LayoutEventRegistration::TextLayoutReport => {
                if is_text(document, node) {
                    document.set_text_layout_reporting(node, now);
                }
            }
        }
    }

    fn exposure_follows(&mut self, node: terminus::link::NodeId, before: (bool, bool)) {
        let Self {
            document, exposure, ..
        } = self;
        let document = document.as_mut().expect(DOCUMENT_EXISTS);
        let now @ (registered, hears) = exposure_rows(document, node);
        if now != before {
            exposure.set_registered(document, node, registered, hears);
        }
    }

    fn forget_realm(&mut self, key: ListenerKey) {
        self.apply_pending_last_removals();
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let changes: SmallVec<[(Arc<str>, RegistrationChange); 4]> = document
            .remove_listeners_of(key)
            .into_iter()
            .map(|name| (name, RegistrationChange::LastRemoved))
            .collect();
        self.publish_route_changes(&changes);
    }

    fn apply_pending_last_removals(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let removals = document.take_pending_last_removals();
        if removals.is_empty() {
            return;
        }
        self.publish_route_changes(&removals);
    }

    fn publish_route_changes<N: RouteName>(&self, changes: &[(N, RegistrationChange)]) {
        for (index, (name, change)) in changes.iter().enumerate() {
            if *change == RegistrationChange::Unchanged {
                continue;
            }
            let (pending, rest) = (&changes[index..], &changes[index + 1..]);
            self.publish_route_change(
                name,
                |name| self.was_registered_before(pending, name),
                |name| self.was_registered_before(rest, name),
            );
        }
    }

    fn was_registered_before<N: RouteName>(
        &self,
        unapplied: &[(N, RegistrationChange)],
        name: &str,
    ) -> bool {
        unapplied
            .iter()
            .find(|(pending, change)| {
                pending.as_str() == name && *change != RegistrationChange::Unchanged
            })
            .map_or_else(
                || self.registered(name),
                |(_, change)| *change == RegistrationChange::LastRemoved,
            )
    }

    fn publish_route_change<N: RouteName>(
        &self,
        name: &N,
        before: impl Fn(&str) -> bool,
        after: impl Fn(&str) -> bool,
    ) {
        let name_str = name.as_str();
        if let Some(routed) = self.route_moved(name_str, &before, &after) {
            self.outbox.set_name_routed(name.to_arc(), routed);
        }
        if name_str == DOUBLE_CLICK_EVENT
            && let Some(routed) = self.route_moved(CLICK_EVENT, &before, &after)
        {
            self.outbox.set_name_routed(Arc::from(CLICK_EVENT), routed);
        }
    }

    fn route_moved(
        &self,
        name: &str,
        before: &impl Fn(&str) -> bool,
        after: &impl Fn(&str) -> bool,
    ) -> Option<bool> {
        let was = self.routes_given(name, before);
        let now = self.routes_given(name, after);
        (was != now).then_some(now)
    }

    fn sync_names(&mut self) {
        self.apply_pending_last_removals();
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let changes = document.take_observed_name_changes();
        for (name, live) in changes {
            self.change_names(&[name], |slot| {
                if live {
                    slot.observed.insert(name);
                } else {
                    slot.observed.remove(name);
                }
            });
        }
    }

    fn change_names(&mut self, names: &[&str], change: impl FnOnce(&mut Self)) {
        let mut moved: SmallVec<[(&str, bool); 6]> = SmallVec::new();
        for &name in names {
            moved.push((name, self.routes(name)));
            if name == DOUBLE_CLICK_EVENT {
                moved.push((CLICK_EVENT, self.routes(CLICK_EVENT)));
            }
        }
        change(self);
        for (name, was) in moved {
            if self.routes(name) != was {
                self.outbox.set_name_routed(Arc::from(name), !was);
            }
        }
    }

    fn routes(&self, name: &str) -> bool {
        self.routes_given(name, &|name| self.registered(name))
    }

    fn routes_given(&self, name: &str, registered: &impl Fn(&str) -> bool) -> bool {
        let wants = |name: &str| registered(name) || self.observed.contains(name);
        wants(name)
            || (name == CLICK_EVENT && wants(DOUBLE_CLICK_EVENT))
            || self.editing_routes(name)
    }

    fn editing_routes(&self, name: &str) -> bool {
        editing::EditingRoute::ALL
            .into_iter()
            .any(|route| self.editing_routes & route.bit() != 0 && route.names().contains(&name))
    }

    fn set_editing_route(&mut self, route: editing::EditingRoute, open: bool) {
        if (self.editing_routes & route.bit() != 0) == open {
            return;
        }
        self.apply_pending_last_removals();
        self.change_names(route.names(), |slot| {
            if open {
                slot.editing_routes |= route.bit();
            } else {
                slot.editing_routes &= !route.bit();
            }
        });
    }

    fn sync_editing_routes(&mut self) {
        let (pointer, focused) = self.document.as_ref().map_or((false, false), |document| {
            (true, document.focused().is_some())
        });
        self.set_editing_route(editing::EditingRoute::Pointer, pointer);
        self.set_editing_route(editing::EditingRoute::Focus, focused);
    }

    fn exposure_attribute_changed(&mut self, node: terminus::link::NodeId) {
        let Self {
            document, exposure, ..
        } = self;
        exposure.attribute_changed(document.as_mut().expect(DOCUMENT_EXISTS), node);
    }

    fn switch_exposure(
        &mut self,
        on: bool,
        send_event: bool,
        nodes: &[terminus::link::NodeId],
    ) -> usize {
        if nodes.is_empty() {
            return 0;
        }
        let Self {
            document, exposure, ..
        } = self;
        exposure.switch(
            document.as_mut().expect(DOCUMENT_EXISTS),
            on,
            send_event,
            nodes,
        )
    }

    fn adopt_metrics(&mut self) {
        let Some(metrics) = *self.metrics.borrow_and_update() else {
            return;
        };
        self.bound = true;
        let document = self.document_mut();
        let viewport = document.viewport_size();
        if viewport.width.to_bits() != metrics.width.to_bits()
            || viewport.height.to_bits() != metrics.height.to_bits()
        {
            document.set_viewport(metrics.width, metrics.height);
        }
        if document.device_pixel_ratio().to_bits() != metrics.device_pixel_ratio.to_bits() {
            document.set_device_pixel_ratio(metrics.device_pixel_ratio);
        }
    }

    fn commit_flush(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<u64, String> {
        self.settle_sheets(thread)?;
        self.adopt_metrics();
        self.sync_names();
        self.sync_content_detection();
        self.sync_container_timing();
        let frame = self.document_mut().commit();
        let mut commit_id = frame.commit_id();
        if self.bound {
            self.held = None;
            self.outbox.publish_frame(frame);
            self.request_wanted_images();
            return Ok(commit_id);
        }
        self.held = Some(frame);
        self.request_wanted_images();
        let mut metrics = self.metrics.clone();
        let token = self.outbox.token().clone();
        thread.wait(async move {
            tokio::select! {
                biased;
                () = token.cancelled() => Err("view was released".to_owned()),
                named = metrics.wait_for(Option::is_some) => named
                    .map(|_| ())
                    .map_err(|_| "the view's seat is gone".to_owned()),
            }
        })?;
        self.adopt_metrics();
        if self.document_mut().needs_render() {
            self.held = Some(self.document_mut().commit());
        }
        if let Some(frame) = self.held.take() {
            commit_id = frame.commit_id();
            self.outbox.publish_frame(frame);
        }
        self.request_wanted_images();
        Ok(commit_id)
    }

    fn settle_sheets(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<(), String> {
        if let Some(error) = &self.failed_sheet {
            return Err(error.clone());
        }
        if self.sheets.is_empty() {
            return Ok(());
        }
        let token = self.outbox.token().clone();
        while !self.sheets.is_empty() {
            let StartupSource { url, answer } = self.sheets.remove(0);
            if let Err(error) = style_sheets::settle_style_sheet(
                self.document_mut(),
                thread,
                &token,
                &url,
                answer,
                None,
            ) {
                self.failed_sheet = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }

    fn commit_if_dirty(&mut self) {
        self.sync_names();
        if !self.sheets.is_empty() || self.failed_sheet.is_some() {
            return;
        }
        self.adopt_metrics();
        if self.document_mut().needs_render() {
            self.sync_content_detection();
            self.sync_container_timing();
            let frame = self.document_mut().commit();
            if self.bound {
                self.held = None;
                self.outbox.publish_frame(frame);
            } else {
                self.held = Some(frame);
            }
            self.request_wanted_images();
        } else if self.bound
            && let Some(frame) = self.held.take()
        {
            self.outbox.publish_frame(frame);
        }
    }

    fn request_wanted_images(&mut self) {
        let wanted = self.document_mut().take_wanted_images();
        if !wanted.is_empty() {
            self.outbox.notify(ViewNotice::RequestImages(wanted));
        }
    }

    fn request_documents(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let requests = document.take_document_requests();
        if !requests.is_empty() {
            self.outbox.notify(ViewNotice::RequestDocuments(requests));
        }
    }

    fn request_playback(&mut self) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let mut requests = self.image_sources.take_playback(document);
        requests.extend(self.videos.take_requests(document));
        if !requests.is_empty() {
            self.outbox.notify(ViewNotice::Playback(requests));
        }
    }

    fn needs_render(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::needs_render)
    }

    fn queue_instant_scroll_events(&mut self) {
        if self
            .document
            .as_ref()
            .is_some_and(LynxDocument::has_instant_scrolls)
        {
            self.instant_scrolls();
        }
    }

    fn has_component_events(&self) -> bool {
        !self.component_events.is_empty()
            || self.document.as_ref().is_some_and(|document| {
                document.has_focus_events() || document.has_pending_selectionchange()
            })
    }

    fn has_pending_content_visibility_changes(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_content_visibility_changes)
    }

    fn update_intersection_observations(&mut self, time: f64) -> bool {
        self.document
            .as_mut()
            .is_some_and(|document| document.update_intersection_observations(time))
    }

    fn has_pending_exposure(&self) -> bool {
        self.exposure.has_changes()
    }

    fn update_resize_observations(&mut self) -> bool {
        self.document
            .as_mut()
            .is_some_and(LynxDocument::update_resize_observations)
    }

    fn has_pending_resize_notifications(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_resize_notifications)
    }

    fn queue_text_layout_events(&mut self) {
        let Self {
            document,
            component_events,
            ..
        } = self;
        let Some(document) = document.as_mut() else {
            return;
        };
        for node in document.take_text_layout_reports() {
            if !document.has_listener(node, LayoutEventRegistration::TextLayoutReport.name()) {
                document.set_text_layout_reporting(node, false);
                continue;
            }
            let (Some(lines), Some(layout)) =
                (document.text_lines(node), document.rounded_layout(node))
            else {
                continue;
            };
            let terminus::link::layout::Layout {
                size,
                padding,
                border,
                ..
            } = layout;
            let width = size.width - padding.left - padding.right - border.left - border.right;
            let height = size.height - padding.top - padding.bottom - border.top - border.bottom;
            component_events.queue_text_layout(node, lines, width, height);
        }
    }

    fn take_font_face_requests(&mut self) -> Vec<terminus::link::FontFaceRequest> {
        self.document
            .as_mut()
            .map(LynxDocument::take_font_face_requests)
            .unwrap_or_default()
    }

    fn note_removal(&mut self) {
        self.removals = self.removals.saturating_add(1);
    }

    fn take_collection_due(&mut self) -> bool {
        if self.removals < REMOVALS_PER_COLLECTION {
            return false;
        }
        self.removals = 0;
        true
    }
}

fn construction_phase<T>(phase: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).map_err(|payload| {
        format!(
            "creating the document panicked while {phase}: {}",
            crate::threads::panic_message(payload.as_ref())
        )
    })
}

pub(crate) struct MainThreadRuntime {
    #[cfg(feature = "devtools")]
    devtools: Option<Rc<devtools::PageSession>>,
    core: Rc<RealmCore>,
    workers: Rc<super::workers::WorkerOwner>,
    slot: Rc<RefCell<DocumentSlot>>,
    screen: ScreenMetrics,
    config: PageConfig,
    entry: String,
    card: Option<String>,
    background_entry: Option<String>,
    last_click: Option<(terminus::link::NodeId, f64)>,
    pointer_editing: editing::PointerEditing,
    text_input: terminus::link::editing::TextInputState,
    text_input_source: crate::editing::TextInputSource,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl MainThreadRuntime {
    pub(crate) fn new(
        js_runtime: &mut ScriptRuntime,
        ingredients: DocumentIngredients,
        metrics: watch::Receiver<Option<Viewport>>,
        outbox: ViewOutbox,
        workers: &super::workers::WorkerFactory,
        thread: crate::jobs::JsThreadHandle,
        mut startup: RealmStartup,
    ) -> Result<
        (
            Self,
            tokio::sync::mpsc::UnboundedReceiver<crate::background::WorkerEvent>,
        ),
        MainThreadError,
    > {
        let config = ingredients.config;
        let sheets = std::mem::take(&mut startup.sheets);
        let slot = DocumentSlot::new(ingredients, sheets, metrics, outbox.clone());
        let host = outbox.host_outbox(outbox.token().clone());
        let (view_workers, incoming) = workers.view(outbox.clone());
        let workers = view_workers.owner(None);
        let document = Rc::new(DocumentHost {
            slot: Rc::clone(&slot),
            outbox,
            thread: thread.clone(),
            workers: Rc::downgrade(&view_workers),
            realms: SharedRealms::default(),
        });
        let (core, ()) = open_realm(
            js_runtime,
            &host,
            thread,
            None,
            RealmNames {
                source: ScriptSource::Main,
                opener: Opener::Realm(OpenerId::PAGE),
            },
            Some(Rc::clone(&document) as Rc<dyn ChildHostModules>),
            crate::canvas::CanvasHost::Document(
                Rc::downgrade(&slot) as std::rc::Weak<dyn crate::canvas::CanvasDocument>
            ),
            |engine| {
                document
                    .install_members(
                        engine,
                        startup_strings(&mut startup),
                        RealmId::Page.listener_key(),
                        None,
                        &RealmTiming::new(OpenerId::PAGE),
                    )
                    .map_err(MainThreadError::into_script_error)?;
                crate::native_module::install(engine, &host, None)
                    .map_err(|error| context_of("installing the native module members", error))?;
                workers
                    .install(engine)
                    .map_err(|error| context_of("installing Worker", error))
            },
        )
        .map_err(|error| MainThreadError::from_engine("opening the MTS realm", error))?;
        let core = Rc::new(core);
        let _ = document.realms.set(Rc::downgrade(&core));
        Ok((
            Self {
                #[cfg(feature = "devtools")]
                devtools: None,
                core,
                workers,
                slot,
                last_click: None,
                pointer_editing: editing::PointerEditing::default(),
                text_input: terminus::link::editing::TextInputState::default(),
                text_input_source: crate::editing::TextInputSource::default(),
                screen: startup.screen,
                config,
                entry: startup.entry,
                card: startup.card,
                background_entry: startup.background_entry,
            },
            incoming,
        ))
    }

    pub(crate) fn core(&self) -> &RealmCore {
        &self.core
    }

    #[cfg(test)]
    pub(crate) fn child_realms(&self) -> usize {
        self.core.children.len()
    }

    #[cfg(test)]
    pub(crate) fn live_workers(&self) -> usize {
        self.workers.live_workers()
    }

    pub(crate) fn dispatch_worker_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: crate::background::WorkerKey,
        payload: crate::background::WorkerPayload,
    ) -> Result<(), MainThreadError> {
        use crate::background::WorkerPayload;
        let child_owner = self.workers.child_owner(key);
        let owner = child_owner.as_ref().unwrap_or(&self.workers);
        let failed = matches!(payload, WorkerPayload::Failed(_));
        let source = matches!(
            payload,
            WorkerPayload::Errored(_) | WorkerPayload::Failed(_)
        )
        .then(|| owner.source_of(key))
        .flatten();
        if matches!(payload, WorkerPayload::Closed | WorkerPayload::Failed(_)) {
            owner.forget(key);
        }
        let (kind, data, location) = match payload {
            WorkerPayload::Message(data) => ("message", data, None),
            WorkerPayload::Closed => ("closed", HostValue::Undefined, None),
            WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                let kind = if failed { "failed" } else { "error" };
                let data = HostValue::String(error.message.to_string());
                let location = error.location.clone();
                if let Some(source) = source {
                    owner.report_failure(source, failed, error);
                }
                (kind, data, location)
            }
        };
        let child = child_owner.as_ref().and_then(|owner| owner.realm());
        let location = location.as_ref();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a key `createWorker` handed out, at most 2^53"
        )]
        let key = key.get() as f64;
        let arguments = [
            HostArgument::Number(key),
            HostArgument::String(kind),
            data.as_argument(),
            HostArgument::String(location.and_then(|l| l.source.as_deref()).unwrap_or("")),
            HostArgument::Number(f64::from(location.and_then(|l| l.line).unwrap_or(0))),
            HostArgument::Number(f64::from(location.and_then(|l| l.column).unwrap_or(0))),
        ];
        let called = match child {
            None => self.core.engine.call_module_export(
                js_runtime,
                Export::DispatchWorkerEvent,
                &arguments,
            ),
            Some(child) => self
                .core
                .children
                .call_module_export(js_runtime, child, Export::DispatchWorkerEvent, &arguments)
                .unwrap_or(Ok(false)),
        }
        .map_err(|error| MainThreadError::from_engine("delivering a worker event", error));
        let finished = self.finish_batch(js_runtime, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    pub(crate) fn deliver_frame_load_metrics(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        card: OpenerId,
        json: &str,
    ) -> Result<(), MainThreadError> {
        let Some(child) = self.slot.borrow().card_realms.get(&card).copied() else {
            return Ok(());
        };
        let called = self
            .core
            .children
            .call_module_export(
                js_runtime,
                child,
                Export::LoadMetrics,
                &[HostArgument::String(json)],
            )
            .unwrap_or(Ok(false))
            .map_err(|error| {
                MainThreadError::from_engine("delivering a frame's loadmetrics", error)
            });
        let finished = self.finish_batch(js_runtime, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    pub(crate) fn settle_entry(&mut self) -> Deliveries {
        let time = self.timeline_milliseconds();
        let mut slot = self.slot.borrow_mut();
        slot.request_documents();
        slot.request_playback();
        slot.sync_editing_routes();
        slot.commit_if_dirty();
        slot.queue_instant_scroll_events();
        slot.publish_text_input_state(&mut self.text_input);
        slot.queue_text_layout_events();
        let mut owed = Deliveries::NONE;
        if slot.update_resize_observations() || slot.has_pending_resize_notifications() {
            owed = owed.with(Deliveries::RESIZE);
        }
        if slot.update_intersection_observations(time) || slot.has_pending_exposure() {
            owed = owed.with(Deliveries::INTERSECTION);
        }
        if slot.has_pending_content_visibility_changes() {
            owed = owed.with(Deliveries::CONTENT_VISIBILITY);
        }
        if slot.has_component_events() && !slot.needs_render() {
            owed = owed.with(Deliveries::COMPONENT_EVENTS);
        }
        owed
    }

    pub(crate) fn commit_if_dirty(&mut self) {
        self.slot.borrow_mut().commit_if_dirty();
    }

    pub(crate) fn apply_page_update(
        &mut self,
        js: &mut ScriptRuntime,
        update: &crate::link::PageUpdate,
    ) -> Result<(), MainThreadError> {
        use crate::link::PageUpdate;

        let (export, arguments): (Export, &[HostArgument<'_>]) = match update {
            PageUpdate::Reload {
                data,
                processor_name,
            } => (
                Export::Reload,
                &[
                    HostArgument::String(data),
                    HostArgument::String(processor_name),
                ],
            ),
            PageUpdate::Data {
                data,
                processor_name,
                reset,
            } => (
                Export::UpdateData,
                &[
                    HostArgument::String(data),
                    HostArgument::String(processor_name),
                    HostArgument::Boolean(*reset),
                ],
            ),
            PageUpdate::GlobalProps(data) => {
                (Export::UpdateGlobalProps, &[HostArgument::String(data)])
            }
            PageUpdate::GlobalEvent { name, arguments } => (
                Export::SendGlobalEvent,
                &[HostArgument::String(name), HostArgument::String(arguments)],
            ),
        };
        let called = self
            .core
            .engine
            .call_module_export(js, export, arguments)
            .map_err(|error| MainThreadError::from_engine("updating page data", error));
        let finished = self.finish_batch(js, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    pub(crate) fn sync_animation_clock(&mut self, now: f64) {
        self.slot
            .borrow_mut()
            .document_mut()
            .sync_animation_clock(now);
    }

    #[cfg(test)]
    pub(crate) fn animation_clock(&self) -> f64 {
        self.slot
            .borrow()
            .document
            .as_ref()
            .expect(DOCUMENT_EXISTS)
            .animation_clock()
    }

    fn timeline_milliseconds(&self) -> f64 {
        self.slot.borrow().timeline_milliseconds
    }

    pub(crate) fn begin_frame(&mut self, now: f64) {
        let mut slot = self.slot.borrow_mut();
        slot.timeline_milliseconds = now * 1000.0;
        let document = slot.document_mut();
        let _ = document.advance_animations(now);
        document.resume_relevance();
    }

    pub(crate) fn vsync(
        &mut self,
        js: &mut ScriptRuntime,
        milliseconds: f64,
    ) -> Result<(), MainThreadError> {
        self.slot.borrow_mut().timeline_milliseconds = milliseconds;
        self.core
            .engine
            .call_module_export(
                js,
                Export::BeginFrame,
                &[HostArgument::Number(milliseconds)],
            )
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine("delivering animation callbacks", error))
    }

    pub(crate) fn deliver_module_callback(
        &mut self,
        js: &mut ScriptRuntime,
        call: u64,
        index: u32,
        arguments: Option<&str>,
    ) -> Result<(), MainThreadError> {
        crate::native_module::deliver(&self.core.engine, js, call, index, arguments).map_err(
            |error| MainThreadError::from_engine("running a native module callback", error),
        )
    }

    pub(crate) fn adopt_scroll_offsets(
        &mut self,
        entries: impl Iterator<Item = (terminus::link::NodeId, crate::link::ScrollEntry)>,
    ) {
        let mut slot = self.slot.borrow_mut();
        let hears_scroll = slot.listens(SCROLL_EVENT);
        let hears_end = slot.listens(SCROLL_END_EVENT);
        let DocumentSlot {
            document,
            component_events,
            scrolling,
            ..
        } = &mut *slot;
        let document = document.as_mut().expect(DOCUMENT_EXISTS);
        let frame = document.committed_frame();
        let mut recenter = false;
        let mut moved: SmallVec<[terminus::link::NodeId; 4]> = SmallVec::new();
        for (node, entry) in entries {
            if let Some(handled) = entry.request {
                document.acknowledge_auto_scroll(node, handled);
            }
            if let Some(pending) = document.pending_scroll_request(node) {
                let Some(handled) = entry.request.filter(|handled| *handled >= pending) else {
                    continue;
                };
                document.acknowledge_scroll_request(node, handled);
            }
            let before = document.scroll_offset(node);
            let unapplied = document.scroll_anchor_shift(node) - entry.anchor_shift;
            let applied = document.scroll_to(node, entry.offset + unapplied);
            document.set_scroll_reach(node, entry.reach);
            if applied != before {
                if hears_scroll {
                    component_events.queue_scroll(node, SCROLL_EVENT, applied - before);
                }
                if !scrolling.contains(&node) {
                    scrolling.push(node);
                }
            }
            if entry.rest.is_some()
                && let Some(index) = scrolling.iter().position(|&held| held == node)
            {
                scrolling.swap_remove(index);
                if hears_end {
                    component_events.queue_scroll(
                        node,
                        SCROLL_END_EVENT,
                        terminus::link::Vector2D::zero(),
                    );
                }
            }
            moved.push(node);
            recenter |= frame.as_ref().is_some_and(|frame| {
                frame.slot_of(node).is_some_and(|index| {
                    frame.scroll_slots()[index as usize].recenter_due(entry.offset)
                })
            });
        }
        if !scrolling.is_empty() {
            scrolling.retain(|node| document.get(*node).is_some());
        }
        if recenter {
            document.note_scroll_windows_stale();
        }
        document.advance_scroll_timelines(&moved);
        document.redetermine_scrolled_fallbacks();
    }

    pub(crate) fn apply_image_events(&mut self, events: &[terminus::link::ImageEvent]) {
        let mut slot = self.slot.borrow_mut();
        let sources = slot.image_sources.clone();
        let videos = slot.videos.clone();
        let outcomes = apply_image_events(slot.document_mut(), &sources, events);
        slot.component_events.extend_images(outcomes);
        let owed = apply_video_reports(slot.document_mut(), &videos, events);
        slot.component_events.extend(owed);
    }

    pub(crate) fn needs_render(&self) -> bool {
        self.slot.borrow().needs_render()
    }

    pub(crate) fn queue_instant_scroll_events(&mut self) {
        self.slot.borrow_mut().queue_instant_scroll_events();
    }

    pub(crate) fn has_component_events(&self) -> bool {
        self.slot.borrow().has_component_events()
    }

    fn dispatch_selectionchange(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        timestamp: f64,
    ) -> Vec<MainThreadError> {
        let mut failures = Vec::new();
        if !self.slot.borrow_mut().document_mut().take_selectionchange() {
            return failures;
        }
        let document_node = self.slot.borrow().document_ref().root_node().id();
        self.dispatch(
            js_runtime,
            document_node,
            SELECTION_CHANGE,
            false,
            false,
            timestamp,
            &EventDetail::None,
            &[],
            &mut failures,
        );
        let mut slot = self.slot.borrow_mut();
        let DocumentSlot {
            document,
            text_selection_reports,
            component_events,
            ..
        } = &mut *slot;
        report_selection_changes(
            document.as_ref().expect(DOCUMENT_EXISTS),
            text_selection_reports,
            component_events,
        );
        failures
    }

    fn push_scroll_extras(
        &mut self,
        node: terminus::link::NodeId,
        delta: terminus::link::Vector2D<f32>,
        extras: &mut SmallVec<[(&'static str, HostArgument<'_>); 7]>,
    ) {
        let scroll_box = self.slot.borrow_mut().document_mut().scroll_box(node);
        let (offset, size) = scroll_box.map_or_else(
            || {
                (
                    terminus::link::Vector2D::zero(),
                    terminus::link::Size2D::zero(),
                )
            },
            |scroll_box| (scroll_box.offset, scroll_box.scroll_size),
        );
        for (key, value) in [
            ("scrollTop", offset.y),
            ("scrollLeft", offset.x),
            ("scrollHeight", size.height),
            ("scrollWidth", size.width),
            ("deltaX", delta.x),
            ("deltaY", delta.y),
        ] {
            extras.push((key, HostArgument::Number(f64::from(value))));
        }
        extras.push(("isDragging", HostArgument::Boolean(false)));
    }

    pub(crate) fn dispatch_component_events(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let timestamp = self.timeline_milliseconds();
        let mut failures = self.dispatch_focus_events(js_runtime, timestamp);
        failures.extend(self.dispatch_selectionchange(js_runtime, timestamp));
        let events = self.slot.borrow().component_events.take();
        for event in &events {
            let lines_text: String;
            let mut extras: SmallVec<[(&'static str, HostArgument<'_>); 7]> = SmallVec::new();
            let (target, name, composed) = match *event {
                ComponentEvent::Image(ImageOutcome::Loaded {
                    node,
                    width,
                    height,
                }) => {
                    let (width, height) = {
                        let mut slot = self.slot.borrow_mut();
                        let document = slot.document_mut();
                        if is_svg(document, node) {
                            document
                                .bounding_client_rect(node)
                                .map_or((0.0, 0.0), |rect| {
                                    (f64::from(rect.size.width), f64::from(rect.size.height))
                                })
                        } else {
                            (f64::from(width), f64::from(height))
                        }
                    };
                    extras.push(("width", HostArgument::Number(width)));
                    extras.push(("height", HostArgument::Number(height)));
                    (node, LOAD_EVENT, false)
                }
                ComponentEvent::Image(ImageOutcome::Failed { node }) => {
                    if is_svg(self.slot.borrow_mut().document_mut(), node) {
                        continue;
                    }
                    (node, ERROR_EVENT, false)
                }
                ComponentEvent::Image(ImageOutcome::StartPlay { node, picture }) => {
                    if !self.slot.borrow().image_sources.autoplays(picture) {
                        continue;
                    }
                    (node, START_PLAY_EVENT, false)
                }
                ComponentEvent::Image(ImageOutcome::Played { node, name })
                | ComponentEvent::Plain { node, name } => (node, name, false),
                ComponentEvent::Frame { node, connected } => {
                    let name = if connected {
                        FRAME_CONNECTED_EVENT
                    } else {
                        FRAME_DISCONNECTED_EVENT
                    };
                    (node, name, false)
                }
                ComponentEvent::Scroll { node, name, delta } => {
                    self.push_scroll_extras(node, delta, &mut extras);
                    (node, name, false)
                }
                ComponentEvent::Detailed {
                    node,
                    name,
                    ref detail,
                } => {
                    push_detail_extras(detail, &mut extras);
                    (node, name, true)
                }
                ComponentEvent::TextLayout {
                    node,
                    ref lines,
                    width,
                    height,
                } => {
                    lines_text = text_lines_json(lines);
                    push_text_layout_extras(lines.len(), &lines_text, width, height, &mut extras);
                    (node, TEXT_LAYOUT_EVENT, false)
                }
            };
            self.dispatch(
                js_runtime,
                target,
                name,
                false,
                composed,
                timestamp,
                &EventDetail::None,
                &extras,
                &mut failures,
            );
            if matches!(
                *event,
                ComponentEvent::Frame {
                    connected: false,
                    ..
                }
            ) {
                self.core.children.release_unneeded();
            }
        }
        failures
    }

    pub(crate) fn send_to_worker(
        &self,
        key: crate::background::WorkerKey,
        message: crate::background::WorkerMessage,
    ) {
        self.workers.post(key, message);
    }

    pub(crate) fn with_document<T>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> T) -> T {
        let mut slot = self.slot.borrow_mut();
        probe(slot.document_mut())
    }

    pub(crate) fn dispatch_input_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        payload: &InputEventPayload,
        release: Option<terminus::link::NodeId>,
        failures: &mut Vec<MainThreadError>,
    ) -> bool {
        if let Some(release) = release {
            return self.dispatch_click(js_runtime, target, release, payload, failures);
        }
        if name == TAP_EVENT {
            self.tap_shadow_content(target, &payload.detail, payload.timestamp);
        }
        self.dispatch(
            js_runtime,
            target,
            name,
            true,
            true,
            payload.timestamp,
            &payload.detail,
            &[],
            failures,
        )
    }

    fn dispatch_click(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        down: terminus::link::NodeId,
        release: terminus::link::NodeId,
        payload: &InputEventPayload,
        failures: &mut Vec<MainThreadError>,
    ) -> bool {
        let (target, click, double) = {
            let slot = self.slot.borrow();
            let Some(document) = slot.document.as_ref() else {
                return false;
            };
            (
                document.nearest_common_event_ancestor(down, release),
                slot.listens(CLICK_EVENT),
                slot.listens(DOUBLE_CLICK_EVENT),
            )
        };
        let Some(target) = target else {
            self.last_click = None;
            return false;
        };
        let at = payload.timestamp;
        let clicked = click
            && self.dispatch(
                js_runtime,
                target,
                CLICK_EVENT,
                true,
                true,
                at,
                &payload.detail,
                &[],
                failures,
            );
        let completes = self
            .last_click
            .take_if(|(last, then)| *last == target && at - *then <= DOUBLE_CLICK_MILLISECONDS)
            .is_some();
        if !completes {
            self.last_click = Some((target, at));
            return clicked;
        }
        let doubled = double
            && self.dispatch(
                js_runtime,
                target,
                DOUBLE_CLICK_EVENT,
                true,
                true,
                at,
                &payload.detail,
                &[],
                failures,
            );
        clicked || doubled
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the event's two flags, as `terminus::link::Document::event_steps` takes them"
    )]
    fn dispatch(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        bubbles: bool,
        composed: bool,
        timestamp: f64,
        detail: &EventDetail,
        extras: Extras<'_>,
        failures: &mut Vec<MainThreadError>,
    ) -> bool {
        self.dispatch_checkpointed(
            js_runtime,
            target,
            name,
            bubbles,
            composed,
            timestamp,
            detail,
            extras,
            Checkpoint::Each,
            failures,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the dispatch's own arguments, and where its calls checkpoint"
    )]
    fn dispatch_checkpointed(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        bubbles: bool,
        composed: bool,
        timestamp: f64,
        detail: &EventDetail,
        extras: Extras<'_>,
        mut checkpoint: Checkpoint<'_>,
        failures: &mut Vec<MainThreadError>,
    ) -> bool {
        let before = failures.len();
        let realms = &*self.core;
        let ran = run_dispatch(
            &self.slot,
            realms,
            &Dispatch {
                target,
                name,
                bubbles,
                composed,
                timestamp,
            },
            &DispatchDetail::Routed { detail, extras },
            |key, arguments| {
                let realm = RealmId::from_listener_key(key);
                let called = match &mut checkpoint {
                    Checkpoint::Each => realms.call_module_export(
                        js_runtime,
                        realm,
                        Export::DispatchEvent,
                        arguments,
                    ),
                    Checkpoint::Deferred(called) => {
                        if !called.contains(&realm) {
                            called.push(realm);
                        }
                        realms.call_module_export_before_operation(
                            js_runtime,
                            realm,
                            Export::DispatchEvent,
                            arguments,
                        )
                    }
                };
                match called {
                    None => false,
                    Some(Ok(called)) => called,
                    Some(Err(error)) => {
                        failures.push(MainThreadError::from_engine("delivering an event", error));
                        false
                    }
                }
            },
        );
        let Some(called) = ran else {
            return false;
        };
        if matches!(checkpoint, Checkpoint::Each)
            && let Err(error) = self.finish_batch(js_runtime, failures.len() == before)
        {
            failures.push(error);
        }
        called
    }

    fn tap_shadow_content(
        &mut self,
        target: terminus::link::NodeId,
        detail: &EventDetail,
        timestamp: f64,
    ) {
        let mut slot = self.slot.borrow_mut();
        let document = slot.document_mut();
        if !document.observes_event(TAP_EVENT) || document.shadow_root_of(target).is_none() {
            return;
        }
        document.dispatch_element_event(target, TAP_EVENT, detail, true, false, timestamp);
    }

    #[cfg(test)]
    pub(crate) fn dispatch_for_test(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        position: terminus::link::Point2D<f32>,
    ) -> Result<bool, Vec<MainThreadError>> {
        let mut failures = Vec::new();
        let called = self.dispatch_input_event(
            js_runtime,
            target,
            name,
            &InputEventPayload {
                detail: terminus::link::event::EventDetail::Pointer {
                    position,
                    id: 1,
                    device: terminus::link::input::PointerKind::Touch,
                    button: 0,
                    buttons: 0,
                },
                ..InputEventPayload::default()
            },
            None,
            &mut failures,
        );
        if failures.is_empty() {
            Ok(called)
        } else {
            Err(failures)
        }
    }

    pub(crate) fn dispatch_content_visibility_changes(
        &mut self,
        js: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let changes = self
            .slot
            .borrow_mut()
            .document_mut()
            .take_content_visibility_changes();
        let timestamp = self.timeline_milliseconds();
        let mut failures = Vec::new();
        for change in changes {
            self.dispatch(
                js,
                change.node,
                CONTENT_VISIBILITY_AUTO_STATE_CHANGE_EVENT,
                true,
                false,
                timestamp,
                &EventDetail::ContentVisibility {
                    skipped: change.skipped,
                },
                &[],
                &mut failures,
            );
        }
        failures
    }

    #[cfg(test)]
    pub(crate) fn update_intersection_observations(&mut self) -> bool {
        let time = self.timeline_milliseconds();
        self.slot
            .borrow_mut()
            .update_intersection_observations(time)
    }

    #[cfg(test)]
    pub(crate) fn has_pending_exposure(&self) -> bool {
        self.slot.borrow().has_pending_exposure()
    }

    pub(crate) fn notify_intersection_observers(
        &mut self,
        js: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        self.slot
            .borrow_mut()
            .document_mut()
            .notify_intersection_observers();
        let mut changes = self.slot.borrow().exposure.take_changes();
        if changes.is_empty() {
            return Vec::new();
        }
        let timestamp = self.timeline_milliseconds();
        let mut failures = Vec::new();
        for change in changes.iter().filter(|change| change.element_event) {
            let name = if change.appeared {
                APPEAR_EVENT
            } else {
                DISAPPEAR_EVENT
            };
            self.dispatch(
                js,
                change.node,
                name,
                false,
                false,
                timestamp,
                &EventDetail::Exposure,
                &[],
                &mut failures,
            );
        }
        let records_from = failures.len();
        let late = self.slot.borrow().exposure.take_changes();
        debug_assert!(late.iter().all(|change| !change.element_event));
        changes.extend(late);
        let mut realms = SmallVec::new();
        for change in &changes {
            let Some(id) = &change.exposure_id else {
                continue;
            };
            let extras = [
                ("exposureID", HostArgument::String(id)),
                (
                    "exposureScene",
                    HostArgument::String(change.scene.as_deref().unwrap_or("")),
                ),
                ("appeared", HostArgument::Boolean(change.appeared)),
            ];
            self.dispatch_checkpointed(
                js,
                change.node,
                EXPOSURE_RECORD_EVENT,
                false,
                false,
                timestamp,
                &EventDetail::None,
                &extras,
                Checkpoint::Deferred(&mut realms),
                &mut failures,
            );
        }
        for realm in realms {
            if let Some(Err(error)) = self.core.checkpoint_after_calls(js, realm) {
                failures.push(MainThreadError::from_engine("delivering an event", error));
            }
        }
        let recorded = failures.len() == records_from;
        if let Err(error) = self.finish_batch(js, recorded) {
            failures.push(error);
        }
        failures
    }

    #[cfg(test)]
    pub(crate) fn update_resize_observations(&mut self) -> bool {
        self.slot.borrow_mut().update_resize_observations()
    }

    #[cfg(test)]
    pub(crate) fn has_pending_resize_notifications(&self) -> bool {
        self.slot.borrow().has_pending_resize_notifications()
    }

    pub(crate) fn notify_resize_observers(
        &mut self,
        js: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let resized = {
            let mut slot = self.slot.borrow_mut();
            let DocumentSlot {
                document,
                layout_change,
                ..
            } = &mut *slot;
            let document = document.as_mut().expect(DOCUMENT_EXISTS);
            document.notify_resize_observers();
            layout_change.take_resized(document)
        };
        let timestamp = self.timeline_milliseconds();
        let mut failures = Vec::new();
        for (target, node) in resized {
            let rect = {
                let mut slot = self.slot.borrow_mut();
                let DocumentSlot {
                    document,
                    layout_change,
                    ..
                } = &mut *slot;
                let document = document.as_mut().expect(DOCUMENT_EXISTS);
                if document.get(node).is_none() || document.get(target).is_none() {
                    continue;
                }
                if !document.has_listener(node, LAYOUT_CHANGE_EVENT) {
                    layout_change.unobserve(document, node);
                    continue;
                }
                document.bounding_client_rect(target)
            };
            let (left, top, width, height) = rect.map_or((0.0, 0.0, 0.0, 0.0), |rect| {
                (
                    f64::from(rect.origin.x),
                    f64::from(rect.origin.y),
                    f64::from(rect.size.width),
                    f64::from(rect.size.height),
                )
            });
            let extras = [
                ("left", HostArgument::Number(left)),
                ("top", HostArgument::Number(top)),
                ("width", HostArgument::Number(width)),
                ("height", HostArgument::Number(height)),
            ];
            self.dispatch(
                js,
                node,
                LAYOUT_CHANGE_EVENT,
                false,
                false,
                timestamp,
                &EventDetail::None,
                &extras,
                &mut failures,
            );
        }
        failures
    }

    #[cfg(test)]
    pub(crate) fn queue_text_layout_events(&mut self) {
        self.slot.borrow_mut().queue_text_layout_events();
    }

    #[cfg(test)]
    pub(crate) fn next_timer_deadline(&mut self) -> Option<ClockInstant> {
        self.core.timers.next_deadline()
    }

    pub(crate) fn run_due_timers(&mut self, js_runtime: &mut ScriptRuntime) -> Vec<ScriptError> {
        self.slot.borrow_mut().run_due_autoplays();
        let own = run_due_timers(&self.core.engine, js_runtime, &self.core.timers);
        let children = self.core.children.run_due_timers(js_runtime);
        if own.is_none() && children.is_none() {
            return Vec::new();
        }
        let mut failures: Vec<ScriptError> = own.into_iter().chain(children).flatten().collect();
        if let Err(error) = self.finish_batch(js_runtime, true) {
            failures.push(error.into_script_error());
        }
        failures
    }

    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        let timers = self.core.timers.next_deadline();
        let autoplays = self.slot.borrow().autoplays.next_deadline();
        match (timers, autoplays) {
            (Some(timers), Some(autoplays)) => Some(timers.min(autoplays)),
            (timers, autoplays) => timers.or(autoplays),
        }
    }

    pub(crate) fn run_boot_module(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        let ScreenMetrics {
            pixel_ratio,
            pixel_width,
            pixel_height,
        } = self.screen;
        let PageConfig {
            default_display_linear,
            default_overflow_visible,
            enable_css_selector,
            enable_js_data_processor,
            enable_css_inheritance,
        } = self.config;
        let entry = serde_json::to_string(&self.entry).expect("a string serializes");
        let background_entry = self.background_entry.as_ref().map_or_else(
            || "undefined".to_owned(),
            |url| serde_json::to_string(url).expect("a string serializes"),
        );
        let boot = format!(
            r#"import {{ lynx, __BobcatConnectBackground, __BobcatInitializeMTS, __BobcatProcessInitData, __BobcatRenderPage }} from "{RUNTIME_MODULE_SPECIFIER}";
import {{ Document, __FlushElementTree }} from "{ELEMENT_MODULE_SPECIFIER}";
import {{ markLoadTiming }} from "{HOST_MODULE_SPECIFIER}";
// Imported for its effect: it installs the timer globals, and a static
// import runs before the entry this module then loads.
import "{TIMER_MODULE_SPECIFIER}";

const config = {{
  defaultDisplayLinear: {default_display_linear},
  defaultOverflowVisible: {default_overflow_visible},
  enableCssSelector: {enable_css_selector},
  enableJSDataProcessor: {enable_js_data_processor},
  enableCssInheritance: {enable_css_inheritance},
}};

// The realm's document, created out of that configuration and held by this
// exported binding for the realm's life. Nothing in the realm releases it: it
// goes when the realm does. The view's own resources — its metrics, its fonts,
// its style pool and its author stylesheets — stay on the host side; the
// first flush below mounts the author sheets, in listed order, before the
// document is styled.
export const document = new Document(config);

__BobcatInitializeMTS({{
  enableJSDataProcessor: config.enableJSDataProcessor,
  systemInfo: {{pixelRatio: {pixel_ratio}, pixelWidth: {pixel_width}, pixelHeight: {pixel_height}}},
}});
// React's entry clears lynx.__initData during initialization. The host's
// first-screen argument belongs to boot, independently of that mutable slot.
let data = lynx.__initData;

// The entry, by the URL the view named it by. A task of the view's owner
// completes this module from the answer the view asked for before this realm
// opened, answered from the response URL the fetcher gave, so this import
// asks the fetcher for nothing, and names the card — the view's card URL, or
// else that response URL — as `__Card__` before the entry's body runs.
//
// The entry is app code. What its evaluation throws is raised again as a
// rejection nothing handles, which the checkpoint of this entry into the
// realm reports, and boot goes on past it: the BTS is connected, the page
// rendered and flushed, as a native MTS renders a page whose script threw.
//
// The view's load begins here (`loadBundleStart`, `vmExecuteStart`), and
// ends at the first flush after the render below.
markLoadTiming("vmExecuteStart", {entry});
try {{ await import({entry}); }} catch (error) {{ void Promise.reject(error); }}
markLoadTiming("vmExecuteEnd");
const {{ Worker, __BobcatWorkerKey }} = await import("bobcat-internal");
data = __BobcatProcessInitData(data);
// The BTS's own entry, by the URL the view named it by, is the BTS's to
// import: this call posts that URL to the BTS in the `initialize` message.
const background = new Worker("{BTS_MODULE_SPECIFIER}", {{ name: "lynx-bg" }});
__BobcatConnectBackground(background, data, {background_entry}, __BobcatWorkerKey(background));

// Queue the flush after jobs already scheduled by the lifecycle hooks.
__BobcatRenderPage(data);
await Promise.resolve().then(() => __FlushElementTree());
"#
        );
        self.evaluate_module(
            js_runtime,
            &boot,
            BOOT_MODULE_SPECIFIER,
            "booting the MTS entry",
        )
    }

    pub(crate) fn run_main_thread_script(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        source_name.clone_into(&mut self.entry);
        self.run_boot_module(js_runtime)?;
        let requested = self.take_module_request();
        debug_assert_eq!(requested, self.entry_module_name().ok());
        self.complete_entry(js_runtime, source_name, &card_entry(source))
            .expect("naming an entry by an absolute URL does not fail")?;
        let finished = self.main_module_finished().map(|_| ());
        let collected = self.finish_batch(js_runtime, finished.is_ok());
        finished.and(collected)
    }

    #[cfg(test)]
    pub(crate) fn run_main_thread_script_over_sheets(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        sheets: Vec<(&str, crate::resource::LoadedSource)>,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        {
            let mut slot = self.slot.borrow_mut();
            let token = slot.outbox.token().clone();
            slot.sheets = sheets
                .into_iter()
                .map(|(url, sheet)| {
                    let (completion, answer) =
                        crate::resource::SourceCompletion::new(token.clone());
                    completion.complete(Ok(sheet));
                    StartupSource {
                        url: url.to_owned(),
                        answer,
                    }
                })
                .collect();
        }
        source_name.clone_into(&mut self.entry);
        self.run_boot_module(js_runtime)?;
        assert_eq!(self.take_module_request(), self.entry_module_name().ok());
        self.complete_entry(js_runtime, source_name, &card_entry(source))
            .expect("naming an entry by an absolute URL does not fail")
    }

    pub(crate) fn take_module_request(&mut self) -> Option<String> {
        self.core.engine.take_module_request()
    }

    pub(crate) fn entry_module_name(&self) -> Result<String, String> {
        super::quickjs::normalize_module_url(BOOT_MODULE_SPECIFIER, &self.entry)
    }

    pub(crate) fn complete_entry(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        url: &str,
        source: &str,
    ) -> Result<Result<(), MainThreadError>, LynxViewError> {
        let booting = |error| MainThreadError::from_engine("booting the MTS entry", error);
        let starting = |error| LynxViewError::Script(booting(error).into_script_error());
        let name = self.entry_module_name().map_err(|message| {
            starting(ScriptError {
                kind: crate::script::ScriptErrorKind::ModuleLoad,
                phase: crate::script::ScriptErrorPhase::ExecuteModule,
                message: format!("the MTS entry {}: {message}", self.entry).into(),
                location: None,
            })
        })?;
        self.core
            .engine
            .call_module_export_before_operation(
                js_runtime,
                Export::InitEntry,
                &[HostArgument::String(self.card.as_deref().unwrap_or(url))],
            )
            .map_err(starting)?;
        Ok(self
            .core
            .engine
            .complete_module(js_runtime, &name, Ok((url, source)))
            .map_err(booting))
    }

    pub(crate) fn take_font_face_requests(&mut self) -> Vec<terminus::link::FontFaceRequest> {
        self.slot.borrow_mut().take_font_face_requests()
    }

    pub(crate) fn register_font_face(&mut self, family: &str, data: terminus::link::FontBlob) {
        self.slot
            .borrow_mut()
            .document_mut()
            .register_font_face(family, data);
    }

    pub(crate) fn main_module_finished(&mut self) -> Result<bool, MainThreadError> {
        self.core
            .engine
            .module_finished()
            .map_err(|error| MainThreadError::from_engine("booting the MTS entry", error))
    }

    pub(crate) fn begin_dispose(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        self.core
            .engine
            .start_module(
                js_runtime,
                &format!(
                    "import {{ __BobcatCloseFrames }} from '{FRAME_MODULE_SPECIFIER}'; \
                     import {{ __BobcatDispose }} from '{RUNTIME_MODULE_SPECIFIER}'; \
                     __BobcatCloseFrames(); await __BobcatDispose();"
                ),
                "bobcat:dispose",
            )
            .map_err(|error| MainThreadError::from_engine("disposing the MTS realm", error))
    }

    pub(crate) fn disposal_finished(&mut self) -> Result<bool, MainThreadError> {
        self.core
            .engine
            .module_finished()
            .map_err(|error| MainThreadError::from_engine("disposing the MTS realm", error))
    }

    fn collect_garbage(&mut self, js_runtime: &mut ScriptRuntime) -> Result<(), MainThreadError> {
        self.slot.borrow_mut().removals = 0;
        self.core
            .engine
            .collect_garbage(js_runtime)
            .map_err(|error| MainThreadError::from_engine("collecting garbage", error))
    }

    fn finish_batch(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        succeeded: bool,
    ) -> Result<(), MainThreadError> {
        let due = succeeded && self.slot.borrow_mut().take_collection_due();
        if due {
            self.collect_garbage(js_runtime)
        } else {
            Ok(())
        }
    }

    pub(crate) fn evaluate_module(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        name: &str,
        phase: &'static str,
    ) -> Result<(), MainThreadError> {
        let result = self
            .core
            .engine
            .start_module(js_runtime, source, name)
            .map_err(|error| MainThreadError::from_engine(phase, error));
        let finished = self.finish_batch(js_runtime, result.is_ok());
        result.and(finished)
    }
}

struct DocumentHost {
    slot: Rc<RefCell<DocumentSlot>>,
    outbox: ViewOutbox,
    thread: crate::jobs::JsThreadHandle,
    workers: std::rc::Weak<super::workers::ViewWorkers>,
    realms: SharedRealms,
}

type SharedRealms = Rc<OnceCell<Weak<RealmCore>>>;

impl DocumentHost {
    fn install_members(
        &self,
        engine: &mut ScriptEngine,
        startup: [(&'static str, Option<String>); 4],
        key: ListenerKey,
        retired: Option<Rc<Cell<bool>>>,
        timing: &Rc<RealmTiming>,
    ) -> Result<(), MainThreadError> {
        install_host_module(
            engine,
            &self.slot,
            self.thread.clone(),
            key,
            retired.clone(),
            timing,
        )?;
        pipelines::install_realm_members(engine, &self.slot, timing)?;
        install_event_members(engine, &self.slot, key, retired, &self.realms, &self.outbox)?;
        style_sheets::install_styles(engine, &self.slot, &self.outbox, self.thread.clone())?;
        install_startup_strings(engine, startup)
    }
}

impl ChildHostModules for DocumentHost {
    fn allocate_opener(&self) -> OpenerId {
        self.outbox.registry().allocate_opener()
    }

    fn install(
        &self,
        engine: &mut ScriptEngine,
        key: ChildKey,
        opener: OpenerId,
        retired: Rc<Cell<bool>>,
    ) -> Result<Box<dyn ChildHostState>, ScriptError> {
        let created = ClockInstant::now();
        let listener_key = RealmId::Child(key).listener_key();
        let timing = RealmTiming::new(opener);
        self.install_members(
            engine,
            STARTUP_STRINGS.map(|name| (name, None)),
            listener_key,
            Some(retired),
            &timing,
        )
        .map_err(MainThreadError::into_script_error)?;
        let view = self.workers.upgrade().ok_or_else(|| {
            crate::threads::platform_script_error(
                "the view's workers have been released".to_owned(),
            )
        })?;
        let workers = view.owner(Some(key));
        workers
            .install(engine)
            .map_err(|error| context_of("installing Worker", error))?;
        timing.set_created(created, ClockInstant::now());
        self.slot.borrow_mut().card_realms.insert(opener, key);
        Ok(Box::new(ChildDocumentState {
            workers: Box::new(workers),
            slot: Rc::clone(&self.slot),
            key: listener_key,
            opener,
        }))
    }
}

fn install(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
        .map_err(|error| MainThreadError::from_engine("installing the host module", error))
}

fn install_reentrant(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl Fn(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_reentrant_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
        .map_err(|error| MainThreadError::from_engine("installing the host module", error))
}

macro_rules! tree_members {
    ($engine:ident, $handle:ident; $(
        fn $name:ident($($arg:ident: $parser:ident),*) |$document:ident| $body:block
    )*) => {$({
        const NAME: &str = concat!("bobcat-internal:host.", stringify!($name));
        let tree = Rc::clone($handle);
        let arity = 0u8 $(+ { let _ = stringify!($arg); 1u8 })*;
        install($engine, stringify!($name), arity, move |arguments| {
            #[allow(unused_mut, reason = "zero-argument members never advance it")]
            let mut index = 0usize;
            $(
                let $arg = $parser(NAME, arguments, index)?;
                index += 1;
            )*
            let _ = (arguments, index);
            let mut handle = borrow_slot(NAME, &tree)?;
            let $document = handle.document_mut();
            $body
        })?;
    })*};
}

fn install_host_module(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    thread: crate::jobs::JsThreadHandle,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
    timing: &Rc<RealmTiming>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        fn createPage() |document| {
            Ok(node_id_value(document.document_element().id()))
        }
        fn createElement(tag: string_argument) |document| {
            let tag = crate::main::tree::document_tag(tag);
            Ok(node_id_value(document.create_element(tag, ())))
        }
        fn parentNode(node: node_id_argument) |document| {
            let parent = document.get(node).and_then(terminus::link::Node::parent_id);
            Ok(parent.map_or(HostValue::Null, node_id_value))
        }
        fn insertBefore(
            parent: node_id_argument,
            child: node_id_argument,
            reference: optional_node_id_argument
        ) |document| {
            validate_insert(document, NAME, parent, child, reference)?;
            document.insert_before(parent, child, reference);
            Ok(HostValue::Undefined)
        }
        fn swapElement(a: node_id_argument, b: node_id_argument) |document| {
            validate_swap(document, NAME, a, b)?;
            document.swap_element(a, b);
            Ok(HostValue::Undefined)
        }
    }

    let tree = Rc::clone(handle);
    install(engine, "removeElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.removeElement";
        let child = node_id_argument(NAME, arguments, 0)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_removable(document, NAME, child)?;
        document.remove_element(child);
        handle.note_removal();
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "replaceElement", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.replaceElement";
        let new_element = node_id_argument(NAME, arguments, 0)?;
        let old_element = node_id_argument(NAME, arguments, 1)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_removable(document, NAME, old_element)?;
        validate_live_element(document, NAME, new_element)?;
        if let Some(parent) = document
            .get(old_element)
            .and_then(terminus::link::Node::parent_id)
        {
            validate_insert(document, NAME, parent, new_element, Some(old_element))?;
            document.insert_before(parent, new_element, Some(old_element));
            document.remove_element(old_element);
            handle.note_removal();
        }
        Ok(HostValue::Undefined)
    })?;

    install_document_members(engine, handle)?;
    install_attribute_members(engine, handle)?;
    install_exposure_members(engine, handle)?;
    install_readback_members(engine, handle)?;

    let tree = Rc::clone(handle);
    let closed = retired.clone();
    install(engine, "dropElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.dropElement";
        if closed.as_ref().is_some_and(|closed| closed.get()) {
            return Ok(HostValue::Undefined);
        }
        let node = node_id_argument(NAME, arguments, 0)?;
        let mut tree = borrow_slot(NAME, &tree)?;
        let document = tree.document_mut();
        validate_removable(document, NAME, node)?;
        if document.is_connected(node) {
            return Err(format!(
                "{NAME} was given a connected element: the element ownership \
                 graph and the tree disagree"
            ));
        }
        document.drop_element(node);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    let realm = Rc::clone(timing);
    install(engine, "flushElementTree", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.flushElementTree";
        let request = FlushRequest {
            realm: &realm,
            pipeline: crate::performance::pipeline_argument(arguments, 0),
            flag: string_argument(NAME, arguments, 1)?,
        };
        borrow_slot(NAME, &tree)?.flush(&thread, &request)?;
        Ok(HostValue::Undefined)
    })?;

    install_nested_page_members(engine, handle, key, retired)
}

fn install_nested_page_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        fn framePage(frame: node_id_argument) |document| {
            validate_live_element(document, NAME, frame)?;
            let page = nested_page(document, frame)
                .ok_or_else(|| format!("{NAME} requires a frame element"))?;
            if frame_depth(document, frame) > MAX_FRAME_DEPTH {
                return Err(format!(
                    "{NAME}: frames nest at most {MAX_FRAME_DEPTH} deep"
                ));
            }
            Ok(node_id_value(page))
        }

        fn resetFrame(frame: node_id_argument) |document| {
            validate_live_element(document, NAME, frame)?;
            reset_frame(document, frame)
                .ok_or_else(|| format!("{NAME} requires a frame element"))?;
            Ok(HostValue::Undefined)
        }
    }

    let tree = Rc::clone(handle);
    install(engine, "closeRealm", 0, move |_arguments| {
        const NAME: &str = "bobcat-internal:host.closeRealm";
        let Some(retired) = &retired else {
            return Err("the page realm cannot close itself".to_owned());
        };
        retired.set(true);
        borrow_slot(NAME, &tree)?.forget_realm(key);
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

fn install_document_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    const NAME: &str = "bobcat-internal:host.createDocument";
    let tree = Rc::clone(handle);
    install(engine, "createDocument", 5, move |arguments| {
        let config = PageConfig {
            default_display_linear: boolean_argument(NAME, arguments, 0)?,
            default_overflow_visible: boolean_argument(NAME, arguments, 1)?,
            enable_css_selector: boolean_argument(NAME, arguments, 2)?,
            enable_js_data_processor: boolean_argument(NAME, arguments, 3)?,
            enable_css_inheritance: boolean_argument(NAME, arguments, 4)?,
        };
        let mut slot = borrow_slot(NAME, &tree)?;
        slot.create_document(config)?;
        Ok(node_id_value(slot.document_ref().root_node().id()))
    })?;

    Ok(())
}

fn install_startup_strings(
    engine: &mut ScriptEngine,
    strings: [(&'static str, Option<String>); 4],
) -> Result<(), MainThreadError> {
    for (name, mut value) in strings {
        install(engine, name, 0, move |_arguments| {
            Ok(value.take().map_or(HostValue::Undefined, HostValue::String))
        })?;
    }
    Ok(())
}

const STARTUP_STRINGS: [&str; 4] = [
    "initData",
    "globalProps",
    "initialProcessor",
    "nativeModuleTable",
];

fn startup_strings(startup: &mut RealmStartup) -> [(&'static str, Option<String>); 4] {
    let [
        init_data,
        global_props,
        initial_processor,
        native_module_table,
    ] = STARTUP_STRINGS;
    [
        (init_data, startup.init_data.take()),
        (global_props, startup.global_props.take()),
        (
            initial_processor,
            Some(std::mem::take(&mut startup.initial_processor)),
        ),
        (
            native_module_table,
            Some(std::mem::take(&mut startup.native_modules)),
        ),
    ]
}

fn install_event_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
    realms: &SharedRealms,
    outbox: &ViewOutbox,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    let closed = retired.clone();
    install(engine, "listenerOpened", 3, move |arguments| {
        const NAME: &str = "bobcat-internal:host.listenerOpened";
        let node = node_id_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        let kinds = kinds_argument(NAME, arguments, 2)?;
        if closed.as_ref().is_some_and(|closed| closed.get()) {
            return Ok(HostValue::Undefined);
        }
        borrow_slot(NAME, &tree)?.open_listener(node, name, kinds, key);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "listenerClosed", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.listenerClosed";
        let node = node_id_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        if retired.as_ref().is_some_and(|retired| retired.get()) {
            return Ok(HostValue::Undefined);
        }
        borrow_slot(NAME, &tree)?.close_listener(node, name, key);
        Ok(HostValue::Undefined)
    })?;

    install_synthetic_event_members(engine, handle, key, realms, outbox)
}

fn install_synthetic_event_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    own: ListenerKey,
    realms: &SharedRealms,
    outbox: &ViewOutbox,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "timelineMilliseconds", 0, move |_arguments| {
        const NAME: &str = "bobcat-internal:host.timelineMilliseconds";
        Ok(HostValue::Number(
            borrow_slot(NAME, &tree)?.timeline_milliseconds,
        ))
    })?;

    let tree = Rc::clone(handle);
    let realms = Rc::clone(realms);
    let outbox = outbox.clone();
    install_reentrant(engine, "dispatchEvent", 6, move |arguments| {
        const NAME: &str = "bobcat-internal:host.dispatchEvent";
        let target = node_id_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        let bubbles = boolean_argument(NAME, arguments, 2)?;
        let composed = boolean_argument(NAME, arguments, 3)?;
        let timestamp = finite_number_argument(NAME, arguments, 4)?;
        let detail = match arguments.get(5) {
            None | Some(HostArgument::Undefined | HostArgument::Null) => None,
            Some(&detail) => Some(detail),
        };
        validate_live_element(borrow_slot(NAME, &tree)?.document_mut(), NAME, target)?;
        let core = realms
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| format!("{NAME}: the view's realms have been released"))?;
        run_dispatch(
            &tree,
            &core,
            &Dispatch {
                target,
                name,
                bubbles,
                composed,
                timestamp,
            },
            &DispatchDetail::Synthetic { own, detail },
            |key, arguments| {
                let realm = RealmId::from_listener_key(key);
                match core.call_module_export_nested(realm, Export::DispatchEvent, arguments) {
                    None => false,
                    Some(Ok(called)) => called,
                    Some(Err(error)) => {
                        let error = MainThreadError::from_engine("delivering an event", error);
                        outbox.engine_event(
                            crate::realm::policy::main_thread(
                                crate::realm::policy::Scene::Listener,
                            )
                            .event_for(error.into_script_error()),
                        );
                        false
                    }
                }
            },
        );
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

fn is_exposure_event(name: &str) -> bool {
    name == EXPOSURE_RECORD_EVENT || name == APPEAR_EVENT || name == DISAPPEAR_EVENT
}

fn exposure_rows(document: &LynxDocument, node: terminus::link::NodeId) -> (bool, bool) {
    (
        document.has_listener(node, EXPOSURE_RECORD_EVENT),
        hears_exposure(document, node),
    )
}

fn hears_exposure(document: &LynxDocument, node: terminus::link::NodeId) -> bool {
    document.has_listener(node, APPEAR_EVENT) || document.has_listener(node, DISAPPEAR_EVENT)
}

struct ChildDocumentState {
    workers: Box<dyn ChildHostState>,
    slot: Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    opener: OpenerId,
}

impl ChildHostState for ChildDocumentState {
    fn is_busy(&self) -> bool {
        self.workers.is_busy()
    }
}

impl Drop for ChildDocumentState {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.slot.try_borrow_mut() {
            slot.card_realms.remove(&self.opener);
            slot.forget_realm(self.key);
        }
    }
}

const UI_METHOD_SUCCESS: f64 = 0.0;
const UI_METHOD_NOT_FOUND: f64 = 3.0;
const UI_METHOD_PARAM_INVALID: f64 = 4.0;
const UI_METHOD_INVALID_STATE: f64 = 7.0;
const UI_METHOD_OPERATION_ERROR: f64 = 8.0;

fn method_answer(outcome: MethodOutcome) -> HostValue {
    HostValue::Number(match outcome {
        MethodOutcome::NotFound => UI_METHOD_NOT_FOUND,
        MethodOutcome::Done => UI_METHOD_SUCCESS,
        MethodOutcome::Failed(MethodError::InvalidParams) => UI_METHOD_PARAM_INVALID,
        MethodOutcome::Failed(MethodError::InvalidState) => UI_METHOD_INVALID_STATE,
        MethodOutcome::Failed(MethodError::Operation) => UI_METHOD_OPERATION_ERROR,
        MethodOutcome::Data(fields) => {
            let mut record = String::with_capacity(fields.len() + 2);
            write_record_field(&mut record, "0");
            record.push_str(&fields);
            return HostValue::String(record);
        }
    })
}

fn install_readback_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "callElementMethod", 3, move |arguments| {
        const NAME: &str = "bobcat-internal:host.callElementMethod";
        let node = node_id_argument(NAME, arguments, 0)?;
        let method = string_argument(NAME, arguments, 1)?;
        let params = string_argument(NAME, arguments, 2)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_live_element(document, NAME, node)?;
        let call = MethodCall {
            name: method,
            params,
        };
        let outcome = match document.invoke_element_method(node, call) {
            MethodOutcome::NotFound => invoke_base_method(document, node, call),
            outcome => outcome,
        };
        Ok(method_answer(outcome))
    })?;

    tree_members! { engine, handle;
        fn scrollRelevanceSpan(scroller: node_id_argument, child: node_id_argument) |document| {
            validate_live_element(document, NAME, scroller)?;
            validate_live_element(document, NAME, child)?;
            let mut record = String::new();
            for value in relevance_span(document, scroller, child).into_iter().flatten() {
                write_record_field(&mut record, &value.to_string());
            }
            Ok(HostValue::String(record))
        }
        fn getComputedStyleMap(
            node: node_id_argument,
            properties: string_argument,
            resolved: flag_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            let filter: Vec<&str> = if properties.is_empty() {
                Vec::new()
            } else {
                properties.split(',').collect()
            };
            let mut record = String::new();
            for (name, value) in document.computed_style_entries(node, &filter, resolved) {
                write_record_field(&mut record, &name);
                write_record_field(&mut record, &value);
            }
            Ok(HostValue::String(record))
        }
    }

    Ok(())
}

fn install_attribute_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    install_attribute_writes(engine, handle)?;

    tree_members! { engine, handle;
        fn setInlineStyles(node: node_id_argument, record: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let declarations = split_style_record(NAME, record)?;
            document.set_inline_style_declarations(node, declarations);
            Ok(HostValue::Undefined)
        }
        fn setInlineStyleProperty(
            node: node_id_argument,
            name: string_argument,
            value: string_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            document.set_inline_style_property(node, name, value);
            Ok(HostValue::Undefined)
        }
        fn supportsStyleProperty(name: string_argument) |document| {
            Ok(HostValue::Boolean(document.supports_style_property(name)))
        }
        fn queryElementIds(
            root: node_id_argument,
            selector: string_argument,
            first_only: flag_argument,
            include_root: flag_argument
        ) |document| {
            validate_live_element(document, NAME, root)?;
            let matches_root =
                include_root && document.matches(root, selector).map_err(|e| e.to_string())?;
            let mut ids = if matches_root { vec![root] } else { Vec::new() };
            if !first_only || ids.is_empty() {
                if first_only {
                    ids.extend(document.query_selector(root, selector).map_err(|e| e.to_string())?);
                } else {
                    ids.extend(document.query_selector_all(root, selector).map_err(|e| e.to_string())?);
                }
            }
            Ok(HostValue::String(ids.into_iter().map(|id| id.to_bits().to_string()).collect::<Vec<_>>().join(",")))
        }
        fn getAttribute(node: node_id_argument, name: string_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let value = element.attribute(name).map(std::borrow::Cow::into_owned);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            let tag = validate_live_element(document, NAME, node)?
                .tag_name()
                .ok_or_else(|| {
                    "bobcat-internal:host.tagName requires a live element tag".to_owned()
                })?;
            Ok(HostValue::String(crate::main::tree::lynx_tag(tag).to_owned()))
        }
        fn attributeNames(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut record = String::new();
            for name in element.attribute_names() {
                write_record_field(&mut record, name);
            }
            Ok(HostValue::String(record))
        }
        fn childIndex(node: node_id_argument) |document| {
            let index = document.get(node).and_then(terminus::link::Node::index_in_parent);
            #[expect(
                clippy::cast_precision_loss,
                reason = "a child list never nears 2^53 entries"
            )]
            Ok(index.map_or(HostValue::Number(-1.0), |index| HostValue::Number(index as f64)))
        }
        fn childElementIds(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut ids = String::new();
            for child in element.children().filter(|child| child.is_element()) {
                if !ids.is_empty() {
                    ids.push(',');
                }
                write!(&mut ids, "{}", child.id().to_bits())
                    .expect("writing to a String cannot fail");
            }
            Ok(HostValue::String(ids))
        }
    }

    Ok(())
}

fn install_attribute_writes(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "setAttribute", 3, move |arguments| {
        const NAME: &str = "bobcat-internal:host.setAttribute";
        let node = node_id_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        let value = string_argument(NAME, arguments, 2)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_live_element(document, NAME, node)?;
        document.set_attribute(node, name, value);
        if is_exposure_attribute(name) {
            handle.exposure_attribute_changed(node);
        } else if name == TIMING_FLAG_ATTRIBUTE && !value.is_empty() {
            handle.pending_flags.push(Arc::from(value));
        } else if name == CONTAINER_TIMING_ATTRIBUTE {
            handle.container_timing_changed(node, Some(value));
        } else if name == CONTAINER_TIMING_IGNORE_ATTRIBUTE {
            handle
                .document_mut()
                .set_container_timing_ignored(node, true);
        }
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "removeAttribute", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.removeAttribute";
        let node = node_id_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_live_element(document, NAME, node)?;
        document.remove_attribute(node, name);
        if is_exposure_attribute(name) {
            handle.exposure_attribute_changed(node);
        } else if name == CONTAINER_TIMING_ATTRIBUTE {
            handle.container_timing_changed(node, None);
        } else if name == CONTAINER_TIMING_IGNORE_ATTRIBUTE {
            handle
                .document_mut()
                .set_container_timing_ignored(node, false);
        }
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

fn install_exposure_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "switchExposure", 3, move |arguments| {
        const NAME: &str = "bobcat-internal:host.switchExposure";
        let on = flag_argument(NAME, arguments, 0)?;
        let send_event = flag_argument(NAME, arguments, 1)?;
        let nodes = node_ids_argument(NAME, arguments, 2)?;
        let queued = borrow_slot(NAME, &tree)?.switch_exposure(on, send_event, &nodes);
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count of registrations is far below f64's exact-integer range"
        )]
        Ok(HostValue::Number(queued as f64))
    })?;

    Ok(())
}

fn split_style_record<'a>(
    function: &str,
    payload: &'a str,
) -> Result<SmallVec<[(&'a str, &'a str); INLINE_DECLARATIONS]>, String> {
    let mut declarations = SmallVec::new();
    let mut rest = payload;
    while !rest.is_empty() {
        let (property, after_property) = take_record_field(function, rest)?;
        let (value, after_value) = take_record_field(function, after_property)?;
        declarations.push((property, value));
        rest = after_value;
    }
    Ok(declarations)
}

fn take_record_field<'a>(function: &str, rest: &'a str) -> Result<(&'a str, &'a str), String> {
    let malformed = || format!("{function} received a malformed style record");
    let separator = rest.find(':').ok_or_else(malformed)?;
    let units: usize = rest[..separator].parse().map_err(|_| malformed())?;
    let body = &rest[separator + 1..];

    let mut counted = 0usize;
    let mut end = 0usize;
    for (offset, character) in body.char_indices() {
        if counted == units {
            end = offset;
            break;
        }
        counted += character.len_utf16();
        end = offset + character.len_utf8();
    }
    if counted != units {
        return Err(malformed());
    }
    Ok((&body[..end], &body[end..]))
}

impl crate::canvas::CanvasDocument for RefCell<DocumentSlot> {
    fn with_text_context(&self, shape: &mut dyn FnMut(&mut terminus::link::TextContext)) -> bool {
        let Ok(mut slot) = self.try_borrow_mut() else {
            return false;
        };
        let Some(document) = slot.document.as_mut() else {
            return false;
        };
        shape(document.text_context_mut());
        true
    }

    fn transfer_control_to_offscreen(
        &self,
        node: terminus::link::NodeId,
    ) -> Result<
        Result<
            crate::paint::canvas_pictures::Transferred,
            terminus::link::html::canvas::InvalidState,
        >,
        String,
    > {
        const NAME: &str = "bobcat-internal:host.canvasTransferControl";
        let mut slot = self
            .try_borrow_mut()
            .map_err(|_| format!("{NAME} cannot re-enter the element tree"))?;
        let slot = &mut *slot;
        let document = slot
            .document
            .as_mut()
            .ok_or_else(|| format!("{NAME} ran before the document exists"))?;
        validate_live_element(document, NAME, node)?;
        Ok(
            crate::paint::canvas_pictures::transfer_control_to_offscreen(
                &slot.outbox,
                document,
                node,
            ),
        )
    }
}

fn borrow_slot<'a>(
    function: &str,
    tree: &'a Rc<RefCell<DocumentSlot>>,
) -> Result<RefMut<'a, DocumentSlot>, String> {
    tree.try_borrow_mut()
        .map_err(|_| format!("{function} cannot re-enter the element tree"))
}

fn validate_live_element<'a>(
    document: &'a LynxDocument,
    function: &str,
    node: terminus::link::NodeId,
) -> Result<&'a terminus::link::Node<()>, String> {
    let node = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if node.is_element() {
        Ok(node)
    } else {
        Err(format!("{function} requires a live element id"))
    }
}

fn validate_removable(
    document: &LynxDocument,
    function: &str,
    node: terminus::link::NodeId,
) -> Result<(), String> {
    let live = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if live.is_document() || live.is_shadow_root() || node == document.document_element().id() {
        Err(format!(
            "{function} cannot remove the document, page root, or a shadow root"
        ))
    } else {
        Ok(())
    }
}

fn validate_insert(
    document: &LynxDocument,
    function: &str,
    parent: terminus::link::NodeId,
    child: terminus::link::NodeId,
    reference: Option<terminus::link::NodeId>,
) -> Result<(), String> {
    validate_live_element(document, function, parent)?;
    validate_removable(document, function, child)?;
    if parent == child || document.is_ancestor(child, parent) {
        return Err(format!("{function} cannot create an element-tree cycle"));
    }
    if reference == Some(child) {
        return Err(format!(
            "{function} requires the reference and child to differ"
        ));
    }
    if let Some(reference) = reference {
        let reference_parent = document
            .get(reference)
            .ok_or_else(|| format!("{function} received a stale reference id"))?
            .parent_id();
        if reference_parent != Some(parent) {
            return Err(format!(
                "{function} requires the reference to be a child of the parent"
            ));
        }
    }
    Ok(())
}

fn validate_swap(
    document: &LynxDocument,
    function: &str,
    a: terminus::link::NodeId,
    b: terminus::link::NodeId,
) -> Result<(), String> {
    if a == b {
        return Err(format!("{function} requires distinct elements"));
    }
    validate_removable(document, function, a)?;
    validate_removable(document, function, b)?;
    if document
        .get(a)
        .and_then(terminus::link::Node::parent_id)
        .is_none()
        || document
            .get(b)
            .and_then(terminus::link::Node::parent_id)
            .is_none()
    {
        return Err(format!("{function} requires attached elements"));
    }
    if document.is_ancestor(a, b) || document.is_ancestor(b, a) {
        return Err(format!("{function} cannot swap an ancestor and descendant"));
    }
    Ok(())
}

fn argument<'a>(arguments: &[HostArgument<'a>], index: usize) -> HostArgument<'a> {
    arguments
        .get(index)
        .copied()
        .unwrap_or(HostArgument::Undefined)
}

const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

#[allow(
    clippy::cast_precision_loss,
    reason = "a packed handle is built to stay inside f64's exact-integer range"
)]
fn packed_node_id(node: terminus::link::NodeId) -> f64 {
    node.to_bits() as f64
}

fn node_id_value(node: terminus::link::NodeId) -> HostValue {
    HostValue::Number(packed_node_id(node))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds and integer checks above make the value a representable handle"
)]
fn node_id_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<terminus::link::NodeId, String> {
    let HostArgument::Number(value) = argument(arguments, index) else {
        return Err(format!("{function} expects a number for argument {index}"));
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= MAX_EXACT_INTEGER {
        return Err(format!(
            "{function} expects a non-negative integer node id for argument {index}"
        ));
    }
    terminus::link::NodeId::from_bits(value as u64)
        .ok_or_else(|| format!("{function} got a number that is no element id: {value}"))
}

fn optional_node_id_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<Option<terminus::link::NodeId>, String> {
    match argument(arguments, index) {
        HostArgument::Undefined | HostArgument::Null => Ok(None),
        _ => node_id_argument(function, arguments, index).map(Some),
    }
}

fn node_ids_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<Vec<terminus::link::NodeId>, String> {
    let malformed = || format!("{function} expects comma-joined node ids for argument {index}");
    let HostArgument::String(ids) = argument(arguments, index) else {
        return Err(malformed());
    };
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    ids.split(',')
        .map(|id| {
            id.parse::<u64>()
                .ok()
                .and_then(terminus::link::NodeId::from_bits)
                .ok_or_else(malformed)
        })
        .collect()
}

fn flag_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<bool, String> {
    match argument(arguments, index) {
        HostArgument::Number(0.0) => Ok(false),
        HostArgument::Number(1.0) => Ok(true),
        _ => Err(format!("{function} expects 0 or 1 for argument {index}")),
    }
}

fn kinds_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<ListenerKinds, String> {
    match argument(arguments, index) {
        HostArgument::Number(value) if value.fract() == 0.0 && (1.0..=7.0).contains(&value) =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an integer in 1..=7"
            )]
            Ok(ListenerKinds::from_bits(value as u8))
        }
        _ => Err(format!(
            "{function} expects the kinds 1 to 7 for argument {index}"
        )),
    }
}

fn finite_number_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<f64, String> {
    match argument(arguments, index) {
        HostArgument::Number(value) if value.is_finite() => Ok(value),
        _ => Err(format!(
            "{function} expects a finite number for argument {index}"
        )),
    }
}

fn boolean_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<bool, String> {
    match argument(arguments, index) {
        HostArgument::Boolean(value) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
    }
}

#[cfg(test)]
mod frame_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod web_text_replication;

#[cfg(test)]
mod worker_tests;
