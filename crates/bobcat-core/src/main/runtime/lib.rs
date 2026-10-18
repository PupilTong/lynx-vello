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

use std::cell::{Cell, RefCell, RefMut};
use std::fmt::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;

use dom::event::{EventDetail, ListenerEdge, ListenerKey, ListenerKinds};
use dom::{MethodCall, MethodError, MethodOutcome, StylePool};
use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tokio::sync::watch;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::clock::ClockInstant;
use crate::esm::{
    ANIMATION_FRAME_MODULE_SPECIFIER, BTS_MODULE_SPECIFIER, ELEMENT_MODULE_SPECIFIER,
    FRAME_MODULE_SPECIFIER, HOST_MODULE_SPECIFIER, RUNTIME_MODULE_SPECIFIER,
    TIMER_MODULE_SPECIFIER, WORKER_CLASS_MODULE_SPECIFIER,
};
use crate::link::{InputEventPayload, ViewNotice, ViewOutbox};
use crate::main::exposure::{Exposure, is_exposure_attribute};
use crate::main::record::write_record_field;
use crate::main::tree::{
    Autoplays, ComponentDetail, ComponentEvent, ComponentEvents, LynxDocument, MAX_FRAME_DEPTH,
    PageConfig, frame_depth, invoke_base_method, is_svg, nested_page, new_document,
};
use crate::realm::child::{ChildHostModules, ChildHostState, ChildKey};
use crate::realm::policy::context_of;
use crate::realm::{RealmCore, open_realm, string_argument};
use crate::script::ScriptError;
use crate::timers::run_due_timers;
use crate::view::{LynxViewError, ScreenMetrics, ScriptSource, StartupSource, Viewport};

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";
const EVENT_DISPATCH_EXPORT: &str = "__BobcatDispatchEvent";

/// The registrant the page realm's event registrations are filed under in
/// the document's listener table. A `ShadowRealm` child's is its
/// [`ChildKey::listener_key`], which is never 0.
const PAGE_REALM: ListenerKey = ListenerKey(0);

/// The event names whose listeners are the document's rather than a path's:
/// every realm with any registration for one of them is called at the
/// document element, as web-core adds its listener for these on `document`
/// rather than on the view's shadow root (`WASMJSBinding.ts:262-282`). The
/// target of either is the document element already (there is no focus
/// model), so a realm whose root is elsewhere — a `frame`'s card — hears it
/// with no step of its own and runs its `global-bindEvent` pass alone.
const DOCUMENT_LEVEL_EVENTS: [&str; 2] = ["keydown", "keyup"];
/// The `bobcat:frame` exports a `frame`'s connection and disconnection are
/// handed to; see [`MainThreadRuntime::dispatch_component_events`].
const FRAME_CONNECTED_EXPORT: &str = "__BobcatFrameConnected";
const FRAME_DISCONNECTED_EXPORT: &str = "__BobcatFrameDisconnected";
/// The `bobcat:runtime` export one global exposure record list is handed to;
/// see [`MainThreadRuntime::send_exposure_records`].
const EXPOSURE_EXPORT: &str = "__BobcatSendExposure";

/// What an element hears when its exposure moves. web-core's names
/// (`ExposureServices.ts:209-220`), dispatched non-bubbling and not composed
/// as web-core dispatches them.
const APPEAR_EVENT: &str = "uiappear";
const DISAPPEAR_EVENT: &str = "uidisappear";

/// CSSOM-View's two scroll events, fired at a scroll container on this
/// thread: `scroll` when an adopted or script-requested offset moved it,
/// `scrollend` when the scroll finished. Non-bubbling, not composed, `{}`
/// detail, delivered as component events; see
/// [`MainThreadRuntime::adopt_scroll_offsets`].
const SCROLL_EVENT: &str = "scroll";
const SCROLL_END_EVENT: &str = "scrollend";

/// UI Events' two click names; see [`MainThreadRuntime::dispatch_click`].
const CLICK_EVENT: &str = "click";
const DOUBLE_CLICK_EVENT: &str = "dblclick";

/// How soon a second `click` at the same target makes a `dblclick`, in
/// milliseconds on the view's timeline.
const DOUBLE_CLICK_MILLISECONDS: f64 = 500.0;

/// What an element's own image source settling is called. Both are web-core's
/// names, which are the browser's (`XImage/ImageEvents.ts`), and both are
/// non-bubbling.
const LOAD_EVENT: &str = "load";
const ERROR_EVENT: &str = "error";

/// The gesture Lynx synthesizes at a release, the one routed event the
/// engine's components hear at their shadow content first; see
/// [`MainThreadRuntime::tap_shadow_content`].
const TAP_EVENT: &str = "tap";

/// The discriminators a detail crosses as: one per [`EventDetail`] *shape*
/// the realm builds a different object out of, never per event name, so the
/// realm reads the discriminator alone and the one dispatch export stays
/// general. Mirrored by the realm's own `DETAIL_*` constants in
/// `element-papi.ts`. (1 was a `load`'s size, which crosses as extras now.)
const DETAIL_INPUT: f64 = 0.0;
const DETAIL_EMPTY: f64 = 2.0;
const DETAIL_EXPOSURE: f64 = 3.0;
const DETAIL_KEY: f64 = 4.0;

/// The bits a key event's modifiers cross as, in [`DETAIL_KEY`]'s last
/// number. Mirrored by `element-papi.ts`.
const MODIFIER_ALT: u8 = 1;
const MODIFIER_CTRL: u8 = 2;
const MODIFIER_META: u8 = 4;
const MODIFIER_SHIFT: u8 = 8;

/// One value of the extras channel: what a Lynx handler reads beside a W3C
/// event's own payload, when it is cheap to compute here (a `load`'s size,
/// a `scroll`'s metrics). The kernel's events stay W3C-shaped
/// ([`EventDetail`]); the realm's Lynx layer turns a dispatch's extras into
/// the Lynx `detail`.
///
/// `Str` borrows a component's string for the length of the dispatch: a
/// [`ComponentDetail`]'s url, content or content id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Extra<'a> {
    Number(f64),
    Bool(bool),
    Str(&'a str),
}

/// One dispatch's extras: names, each with its value. Empty for nearly
/// every event.
type Extras<'a> = &'a [(&'static str, Extra<'a>)];

/// Appends a [`ComponentDetail`]'s strings as extras, under the keys
/// web-core's `<x-markdown>` details carry: `id`; `url`, `content` and
/// `contentId`; `url` and `contentId`. An `id` or `contentId` the component
/// had no value for pushes nothing, so the realm's `detail` has no such key.
fn push_detail_strings<'a>(
    detail: &'a ComponentDetail,
    extras: &mut SmallVec<[(&'static str, Extra<'a>); 7]>,
) {
    let (url, content, id_key, id) = match detail {
        ComponentDetail::Id { id } => (None, None, "id", id),
        ComponentDetail::UrlContent {
            url,
            content,
            content_id,
        } => (Some(url), Some(content), "contentId", content_id),
        ComponentDetail::Url { url, content_id } => (Some(url), None, "contentId", content_id),
    };
    for (key, value) in [("url", url), ("content", content)] {
        if let Some(value) = value {
            extras.push((key, Extra::Str(value)));
        }
    }
    if let Some(id) = id {
        extras.push((id_key, Extra::Str(id)));
    }
}

/// Inline arguments for one dispatch: the six of the header, then an
/// input event's discriminator and eight detail numbers. Extras and touch
/// points spill.
type Arguments<'a> = SmallVec<[HostArgument<'a>; 15]>;

/// One realm one dispatch calls: which, at which node's capture step, and
/// its part of the path, encoded as the call carries it.
struct RealmCall {
    /// The child to call, or `None` for the page realm.
    child: Option<ChildKey>,
    /// The registrant the realm's registrations are filed under.
    key: ListenerKey,
    /// The node at whose capture step the realm is called.
    at: dom::NodeId,
    /// The steps the realm is handed, target first, as comma-joined packed
    /// node ids: each step's node, then the target that step sees.
    nodes: String,
    targets: String,
}

/// The realms a dispatch of `name` over `steps` calls, in the order they are
/// considered — the page realm, then `children` in construction order — each
/// with where it is called and what it is handed; see
/// [`MainThreadRuntime::dispatch`] for the rules. A realm whose call step is
/// not on the path is left out.
///
/// Built while the document is borrowed and before the walk takes a step, so
/// the parts are encoded against the tree the path was computed over, which
/// is the standard's rule: the path is fixed when the dispatch begins.
fn realm_calls(
    slot: &DocumentSlot,
    document: &LynxDocument,
    steps: &dom::event::EventSteps,
    name: &str,
    children: &[ChildKey],
) -> SmallVec<[RealmCall; 2]> {
    let mut calls = SmallVec::new();
    let capture: SmallVec<[&dom::event::EventStep; 16]> =
        steps.steps().iter().filter(|step| step.capture).collect();
    let Some(top) = capture.first().map(|step| step.node) else {
        return calls;
    };
    let on_path = document.listeners_on_path(steps, name);
    let document_level = DOCUMENT_LEVEL_EVENTS.contains(&name);
    let invited = |key: ListenerKey| {
        on_path.contains(&key)
            || document.realm_has_global(key, name)
            || (document_level && document.realm_registration_counts(key, name) != (0, 0))
    };
    // The page realm answers for the document tree, and for a detached tree:
    // its call is at the path's own root, unless that root is a shadow root,
    // where a non-composed event inside a shadow tree ends.
    if invited(PAGE_REALM) && document.shadow_root_of(top).is_none() {
        calls.extend(realm_call(document, &capture, None, PAGE_REALM, top, None));
    }
    for &child in children {
        let key = child.listener_key();
        let Some(&root) = slot.roots.get(&key) else {
            continue;
        };
        if !invited(key) {
            continue;
        }
        let at = if document_level {
            dom::NodeId::DOCUMENT_ELEMENT
        } else {
            root
        };
        let tree = document.shadow_root_of(root);
        calls.extend(realm_call(document, &capture, Some(child), key, at, tree));
    }
    calls
}

/// One [`RealmCall`] at `at`'s capture step, handed the capture steps from
/// there inward that are in the tree `tree` roots (`None`: the document tree
/// or a detached one), target first. `None` when `at` is not on the path.
fn realm_call(
    document: &LynxDocument,
    capture: &[&dom::event::EventStep],
    child: Option<ChildKey>,
    key: ListenerKey,
    at: dom::NodeId,
    tree: Option<dom::NodeId>,
) -> Option<RealmCall> {
    let from = capture.iter().position(|step| step.node == at)?;
    let mut nodes = String::new();
    let mut targets = String::new();
    // The capture pass read backwards is the path, target first, whatever
    // `bubbles` cut from the bubble pass.
    for step in capture[from..]
        .iter()
        .rev()
        .filter(|step| document.shadow_root_of(step.node) == tree)
    {
        if !nodes.is_empty() {
            nodes.push(',');
            targets.push(',');
        }
        write!(nodes, "{}", packed_node_id(step.node)).expect("writing to a String");
        write!(targets, "{}", packed_node_id(step.target)).expect("writing to a String");
    }
    Some(RealmCall {
        child,
        key,
        at,
        nodes,
        targets,
    })
}

/// Appends the extras channel: the count, then each name and its value.
fn encode_extras<'a>(extras: Extras<'a>, arguments: &mut Arguments<'a>) {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a handful of extras, far inside an f64's integers"
    )]
    arguments.push(HostArgument::Number(extras.len() as f64));
    for (name, value) in extras {
        arguments.push(HostArgument::String(name));
        arguments.push(match *value {
            Extra::Number(number) => HostArgument::Number(number),
            Extra::Bool(flag) => HostArgument::Boolean(flag),
            Extra::Str(text) => HostArgument::String(text),
        });
    }
}

/// The W3C `PointerEvent.pointerType` of a device.
fn pointer_type(device: dom::input::PointerKind) -> &'static str {
    match device {
        dom::input::PointerKind::Touch => "touch",
        dom::input::PointerKind::Pen => "pen",
        _ => "mouse",
    }
}

/// The script boundary's encoding of `dom`'s [`EventDetail`]: the
/// discriminator, then the values that kind spends, in the order the realm
/// reads them. `dom`'s detail is the one description of what an event
/// carries; this is only how it crosses, and the object is built in the
/// realm, because its shape is JavaScript's.
///
/// - [`DETAIL_INPUT`] for a pointer, wheel or touch event, eight values: the device position; the
///   wheel delta; W3C `button`, `buttons`, `pointerId` and `pointerType` — each `undefined` for a
///   shape that has none, which is how the realm tells the three apart — then four numbers per
///   touch point. The realm puts the W3C members on the event object.
/// - [`DETAIL_EXPOSURE`] for a `uiappear`/`uidisappear`: nothing. The realm builds web-core's
///   detail (`ExposureServices.ts:202-208`) from the target's own unique id and its `exposure-id`
///   and `exposure-scene` attributes, which are the live element's because
///   [`MainThreadRuntime::dispatch`] resolves the target before it calls the realm.
/// - [`DETAIL_KEY`] for `keydown`/`keyup`: the W3C `key` and `code` strings, borrowed from the
///   detail, `repeat` as a boolean and the modifiers as one bitmask. The realm puts them on the
///   event object itself, as `KeyboardEvent` and web-core do.
/// - [`DETAIL_EMPTY`] for everything else: nothing.
fn encode_detail<'a>(detail: &'a EventDetail, arguments: &mut Arguments<'a>) {
    let (position, delta, pointer, points): (_, _, _, &[dom::event::TouchPoint]) = match detail {
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
        EventDetail::Key {
            key,
            code,
            repeat,
            modifiers,
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
        // `None`, and the engine's own `ContentVisibility`, which never
        // reaches a realm.
        _ => {
            arguments.push(HostArgument::Number(DETAIL_EMPTY));
            return;
        }
    };
    let number = |value: Option<f64>| value.map_or(HostArgument::Undefined, HostArgument::Number);
    arguments.push(HostArgument::Number(DETAIL_INPUT));
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

/// Declarations one `__SetInlineStyles` record carries without touching the
/// heap. Compiled `ReactLynx` records are a handful of properties.
const INLINE_DECLARATIONS: usize = 16;

mod style_sheets;

/// A card's MTS body as the entry `bobcat-source` registers for it:
/// [`crate::esm::MTS_CHUNK_PREAMBLE`], then the body on the preamble's own
/// line, so the body's first line is the module's first.
///
/// Only the seams without a fetcher write an entry this way —
/// [`MainThreadRuntime::run_main_thread_script`] and the crate's own test
/// hosts. A view completes its entry with what the fetcher answered.
pub(crate) fn card_entry(body: &str) -> String {
    let mut module = String::with_capacity(crate::esm::MTS_CHUNK_PREAMBLE.len() + body.len());
    module.push_str(crate::esm::MTS_CHUNK_PREAMBLE);
    module.push_str(body);
    module
}

/// Why constructing or running the engine-owned main-thread runtime failed.
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

/// What one dispatch's listeners threw: one failure per realm call that
/// failed, in the order the calls ran, never empty.
///
/// A dispatch calls each realm that listens on its path once, and a realm
/// whose listener throws does not keep the rest of the walk — the other
/// realms, the components after it — from running, so one dispatch can fail
/// more than once. Every failure is kept, for the caller to report each.
#[derive(Debug)]
pub(crate) struct ListenerFailures(SmallVec<[MainThreadError; 1]>);

impl ListenerFailures {
    /// Every failure, in the order the calls that failed ran.
    pub(crate) fn into_errors(self) -> impl Iterator<Item = MainThreadError> {
        self.0.into_iter()
    }

    /// `failures` as a dispatch's answer: `Ok(called)` when there are none.
    fn or_called(failures: SmallVec<[MainThreadError; 1]>, called: bool) -> Result<bool, Self> {
        if failures.is_empty() {
            Ok(called)
        } else {
            Err(Self(failures))
        }
    }
}

impl fmt::Display for ListenerFailures {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, failure) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{failure}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MainThreadError {}

/// How many removals go by between collections.
///
/// A removal is where a subtree's handles start dying — `ReactLynx` unmounts
/// with `__RemoveElement` and then deletes the snapshot's element list, which
/// drops the last reference to the detached root's handle and, through the
/// child sets under it, to every handle in the subtree — and nothing frees
/// the elements until those handles are finalized. A handle reachable only
/// from a registration it captured is a cycle, which only a collection
/// resolves. `QuickJS` collects on its own only at allocation pressure (its
/// threshold is 1.5× the live size after each collection), so a card that
/// detaches steadily while allocating little keeps dead subtrees for a long
/// time: measured, a 301-node subtree outlived 30 batches at 5 elements of
/// churn per batch, and an idle card never freed it. Counting removals is the
/// cheapest signal that correlates with that garbage; the count is a policy
/// knob, not a measurement.
const REMOVALS_PER_COLLECTION: u32 = 32;

/// The Rust side of what a document is built out of: everything the realm is
/// not told and does not decide.
///
/// The boot module is what creates the document, by constructing a `Document`
/// over the page configuration written into it; the host member behind that
/// constructor takes the rest from here. The view's own resources therefore
/// never become JavaScript values — the realm names its metrics, its fonts and
/// its style pool nowhere.
pub(crate) struct DocumentIngredients {
    /// The metrics the document is created at, as the embedder named them to
    /// `create_lynx_view`. They are what it works at until a painter binds;
    /// a painter that bound first supersedes them at the construction.
    ///
    /// A view's metrics and nothing else. What `SystemInfo` reports is the
    /// screen, which the embedder names separately in
    /// [`ViewSources::screen`](crate::ViewSources::screen).
    pub(crate) viewport: Viewport,
    /// The page configuration the view was built with. The boot module is
    /// written with its five switches as literals, and hands them back to the
    /// `createDocument` that builds the document as five boolean arguments.
    /// What the document is actually built with is therefore the realm's
    /// copy, not this one.
    pub(crate) config: PageConfig,
    /// The fonts and the default family, already validated against a context
    /// of their own — see
    /// [`adopt_text_context`](dom::Document::adopt_text_context). `None` is a
    /// view that named neither, which leaves the document's own lazy context
    /// alone.
    pub(crate) text_context: Option<dom::TextContext>,
    pub(crate) style_pool: Option<Rc<StylePool>>,
}

/// A metrics watch that already holds `viewport`, so the realm built over it
/// is bound from the start.
///
/// The tests that build a realm in place play the painting side as well as
/// the view's, and an unbound `__FlushElementTree` would park on the binding
/// that side never makes. The sender is dropped with the call: a bound slot
/// only ever reads the value.
#[cfg(test)]
pub(crate) fn bound_metrics(viewport: Viewport) -> watch::Receiver<Option<Viewport>> {
    watch::channel(Some(viewport)).1
}

#[cfg(test)]
impl DocumentIngredients {
    /// The ingredients of a view that lists no sheets, fonts or pool — what
    /// a test stages when the document itself is not what it is about.
    pub(crate) fn for_test(viewport: Viewport, config: PageConfig) -> Self {
        Self {
            viewport,
            config,
            text_context: None,
            style_pool: None,
        }
    }
}

/// Everything one realm is opened with, in one value.
///
/// One struct because all of it shares one lifetime: it is handed over exactly
/// once, as the realm opens, and nothing ever updates it.
/// [`LynxView::update_data`](crate::LynxView::update_data),
/// [`update_global_props`](crate::LynxView::update_global_props) and
/// [`reload`](crate::LynxView::reload) reach the realm through
/// `ToMain::PageUpdate` instead, and never touch any of this. The three
/// page-data strings and `native_modules` become one-shot host members the
/// realm alone reads, `entry`, `background_entry` and `screen` are written
/// into the boot module's source, and `sheets` go to the document slot,
/// which the first `__FlushElementTree` settles them out of. The BTS is
/// handed its share by this realm's script, in the `initialize` message boot
/// posts to it: the BTS entry, this realm's own `SystemInfo` and the module
/// table. The entry's source is not here: its answer is a task of the view's
/// owner, which completes the module boot imports the entry as.
pub(crate) struct RealmStartup {
    /// The answers to the author stylesheet requests `create_lynx_view` made,
    /// in the order the view listed them, which is their cascade order. The
    /// first `__FlushElementTree` waits for each and mounts it before the
    /// document enters the style pipeline; nothing waits for them earlier.
    pub(crate) sheets: Vec<StartupSource>,
    /// The screen the realm's `SystemInfo` reports, as the embedder named it
    /// in [`ViewSources::screen`](crate::ViewSources::screen). The BTS's
    /// `SystemInfo` reports the same numbers: boot posts this realm's own
    /// `SystemInfo` to the BTS in its `initialize` message.
    pub(crate) screen: ScreenMetrics,
    /// The view's MTS entry, [`ViewSources::entry`](crate::ViewSources::entry)
    /// as `create_lynx_view` resolved it: always an absolute URL, in its
    /// WHATWG serialization. Boot imports the entry by this string.
    pub(crate) entry: String,
    /// What `__Card__` names, [`ViewSources::card_url`](crate::ViewSources::card_url)
    /// as `create_lynx_view` resolved it: the page's container URL, or `None`
    /// for the entry's own response URL.
    pub(crate) card: Option<String>,
    /// The BTS entry `bobcat:bts` imports, if the view named one: always an
    /// absolute URL `create_lynx_view` resolved, like [`Self::entry`]. Boot
    /// writes it into its own source and posts it to the BTS in the
    /// `initialize` message.
    pub(crate) background_entry: Option<String>,
    /// The host's processor name, page data and global props, as the strings
    /// it passed in. `bobcat:runtime` parses the data and props as JSON and
    /// uses the processor name unchanged.
    pub(crate) initial_processor: String,
    pub(crate) init_data: Option<String>,
    pub(crate) global_props: Option<String>,
    /// The embedder's modules as one `<utf16Length>:<text>` record, two fields
    /// per module: its name, then its method names joined with commas. Empty
    /// for a view built with none. This realm reads it once, as the host
    /// member `nativeModuleTable`, and posts it unread to the BTS in the
    /// `initialize` message, where `bobcat:bts-runtime` builds
    /// `NativeModules` out of it.
    pub(crate) native_modules: String,
}

/// Hand-written rather than derived, because there is no default screen: every
/// embedder names one. What opens a realm from `RealmStartup::default()` is
/// this crate's own tests and benchmarks, and none of them reads `SystemInfo`.
impl Default for RealmStartup {
    fn default() -> Self {
        Self {
            sheets: Vec::new(),
            screen: ScreenMetrics {
                pixel_ratio: 1.0,
                pixel_width: 0.0,
                pixel_height: 0.0,
            },
            // No entry: the seams that boot a realm opened from this name one
            // themselves.
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

/// The realm's document and the ingredients it is built out of, plus the
/// publish seam its commits leave through.
///
/// Filling it is the realm's doing: `createDocument` builds the document out
/// of the configuration the boot module hands it and the Rust-side ingredients
/// below, and the first `__FlushElementTree` mounts the view's author sheets
/// on it, in listed order, before the document is styled for the first time.
/// Nothing empties it again — the slot drops with the view's task, after the
/// realm that named the document has been freed.
///
/// It is also where a view learns its painter's metrics, and so where the
/// binding is decided: the document is created at the create-time viewport
/// and works at it until a painter writes the watch, and a flush before that
/// holds its frame and parks.
struct DocumentSlot {
    /// What a `createDocument` builds from, taken by the first one that runs.
    ingredients: Option<DocumentIngredients>,
    /// The answers to this view's author stylesheet requests that have not
    /// been mounted yet, in the order the view listed them.
    ///
    /// Order of *use* rather than of completion: [`Self::flush`] mounts them
    /// one at a time and in this order, whatever order the fetcher answered
    /// them in, so the cascade order between listed sheets is the listed
    /// order. Non-empty is what keeps [`Self::commit_if_dirty`] from running
    /// the pipeline, so no frame is committed without them.
    sheets: Vec<StartupSource>,
    /// The error the first listed sheet that failed to settle threw, which
    /// every later settle throws again.
    ///
    /// A failed sheet leaves [`Self::sheets`] like any other, so without this
    /// only the first settle would see the failure — and that can be app
    /// code's, an `adoptStyleSheet` or a `__FlushElementTree` inside the
    /// entry's evaluation, which may catch it. Kept here, it reaches boot's
    /// own flush as well, which fails the boot with it whatever the app did.
    /// Like an outstanding sheet, it keeps [`Self::commit_if_dirty`] from
    /// committing.
    failed_sheet: Option<String>,
    document: Option<LynxDocument>,
    /// The device metrics an attached painter names, `None` until one binds.
    ///
    /// Read at the start of [`Self::commit_if_dirty`] and [`Self::flush`],
    /// and polled directly by the wait an unbound flush parks on — never
    /// routed through a command, because no job runs while one is parked.
    metrics: watch::Receiver<Option<Viewport>>,
    /// Whether a painter has ever named its metrics for this view.
    ///
    /// Only the first binding is waited for: a painter that detaches leaves
    /// this set and the last metrics in place, so a later flush publishes
    /// rather than parking.
    bound: bool,
    /// The newest frame committed before the binding, which is not published
    /// yet.
    ///
    /// A frame is held rather than published because a painter composes at
    /// its own size: it has no way to tell that the frame it adopted was
    /// committed for the metrics it has just replaced. The binding is what
    /// releases it — as it is, if the metrics match, or recomputed if they do
    /// not — and a newer commit published once bound drops it, so it is
    /// never published after a frame that superseded it.
    held: Option<Arc<dom::CommittedFrame>>,
    /// The events the document's components owe script: an image's `load`s
    /// and `error`s, from the `image` component, which settles a `src` inside
    /// the `__SetAttribute` that wrote it, and from
    /// [`MainThreadRuntime::apply_image_events`], which settles one from the
    /// painting side's report; and a dialog's `cancel`s and `close`s, from its
    /// UI methods in `callElementMethod`. Held here rather than inside the
    /// document because the `image` component is the far end of it and
    /// reaches nothing else; the runtime drains it once per entry.
    component_events: ComponentEvents,
    /// The autoplay timers of the document's `x-swiper`s, which the
    /// component starts and stops from its reactions. Held here for what
    /// `component_events` is: the runtime waits for the earliest
    /// ([`MainThreadRuntime::next_deadline`]) and runs what came due
    /// ([`MainThreadRuntime::run_due_timers`]). Empty on a page with no
    /// autoplaying swiper.
    autoplays: Autoplays,
    /// The document's exposure registrations and the transitions they owe
    /// script.
    ///
    /// Beside the document rather than inside it because exposure is Lynx's
    /// and `dom` knows nothing of it: each registration is one of `dom`'s
    /// intersection observers, and what those deliver is turned into
    /// exposure here and drained into the realm by the intersection delivery
    /// entry ([`MainThreadRuntime::notify_intersection_observers`]).
    exposure: Exposure,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// The page each `ShadowRealm` child answers as, by the registrant its
    /// event registrations are filed under: the nested page its
    /// `framePage` handed it, which `bobcat:frame-boot` makes the child's
    /// page root. The one fact about a realm a dispatch needs and the
    /// document cannot answer, because which realm renders under which page
    /// is the realms' own business: the step at that page is where the child
    /// is called ([`MainThreadRuntime::dispatch`]). Written by a child's
    /// `framePage`, removed when the child closes or is freed
    /// ([`Self::forget_realm`]); the page realm's root is the document
    /// element, which needs no entry. One entry per live frame card.
    roots: FxHashMap<ListenerKey, dom::NodeId>,
    /// The event names the document's components observe on an element that
    /// exists, as [`dom::Document::take_observed_name_edges`] said when last
    /// drained ([`Self::sync_names`]). With the document's listener
    /// table ([`dom::Document::has_listeners_for`]), what the painter routes
    /// against ([`Self::routes`]).
    observed: FxHashSet<&'static str>,
    /// The scroll containers that have scrolled since the painter last
    /// posted them at rest: an adopted offset moved them, or a script-facing
    /// instant scroll did. Kept whether or not anything listens, so a
    /// `scrollend` listener added mid-scroll still hears the end; a rest
    /// takes a container out, and so does its node being freed. Bounded by
    /// the containers scrolling at once.
    scrolling: SmallVec<[dom::NodeId; 4]>,
    /// Where committed frames leave for the painting side.
    outbox: ViewOutbox,
}

/// What every caller of [`DocumentSlot::document_mut`] relies on, stated at
/// the one place that could observe it failing.
const DOCUMENT_EXISTS: &str = "the boot module creates the document before any card runs";

impl DocumentSlot {
    /// The slot a realm opens with: no document, the inputs the one
    /// `createDocument` will build it from, and the seat it publishes
    /// through.
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
            autoplays: Autoplays::default(),
            exposure: Exposure::new(),
            removals: 0,
            roots: FxHashMap::default(),
            observed: FxHashSet::default(),
            scrolling: SmallVec::new(),
            outbox,
        }))
    }

    /// The realm's document.
    ///
    /// Panicking is what a missing document deserves here rather than an
    /// error every member would have to carry: no JavaScript runs in this
    /// realm before the boot module's first statement creates the document,
    /// and nothing empties the slot until the realm itself is freed.
    fn document_mut(&mut self) -> &mut LynxDocument {
        self.document.as_mut().expect(DOCUMENT_EXISTS)
    }

    /// Turns every `x-swiper` whose autoplay timer has come due. One test on
    /// a page where none autoplays, which is also every page whose document
    /// does not exist yet: a timer is a connected swiper's.
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

    /// Builds the realm's one document out of the configuration the boot
    /// module hands it and the Rust-side ingredients the view was created
    /// with.
    ///
    /// The ingredients are spent by the first call, which is the whole of the
    /// refusal a second one gets: a construction that fails rejects the boot
    /// module's `new Document(config)`, which fails the boot and ends the view,
    /// so nothing asks again.
    ///
    /// It never waits. The view's author sheets are not mounted here: the
    /// first [`Self::flush`] mounts them, which is the first point at which
    /// the document is styled.
    ///
    /// The construction is caught, because this is the one host member that
    /// runs the UA cascade behind a single call: a panic in it is reported
    /// naming the phase that panicked, as the construction failure it is,
    /// where any other host member's panic ends the view as `Panicked`.
    fn create_document(&mut self, config: PageConfig) -> Result<(), String> {
        let Some(ingredients) = self.ingredients.take() else {
            return Err("the realm already created its document".to_owned());
        };
        let DocumentIngredients {
            viewport,
            // The realm's copy is what the document is built with: the boot
            // module was written with this one and handed it back.
            config: _,
            text_context,
            style_pool,
        } = ingredients;
        // The create-time viewport is what the document works at until a
        // painter binds; a painter that bound before the boot module ran its
        // first statement has already named the real one.
        let viewport = match *self.metrics.borrow_and_update() {
            Some(metrics) => {
                self.bound = true;
                metrics
            }
            None => viewport,
        };
        let events = self.component_events.clone();
        let autoplays = self.autoplays.clone();
        let document = construction_phase("building the page", || {
            let mut document = new_document(viewport, config, events, autoplays);
            if let Some(pool) = style_pool {
                document.set_style_pool(pool);
            }
            if let Some(context) = text_context {
                document.adopt_text_context(context);
            }
            document
        })?;
        // Nothing of the components' is published here: a name they observe
        // is routed once an element observing it exists, which the
        // document's edges say at the first commit.
        self.document = Some(document);
        Ok(())
    }

    /// Queues a `scroll` at every container a script-facing instant scroll
    /// moved ([`dom::Document::take_instant_scrolls`]), if something hears
    /// it, and records the container as scrolling. Its `scrollend` comes
    /// from the painter, which carries the request out and then posts the
    /// container at rest — or, during a drag or a fling, once that ends —
    /// so nothing here ends a scroll.
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

    /// Whether anything hears `name`: a realm registration anywhere, in the
    /// document's listener table, or an element of a component that observes
    /// it. Two lookups.
    ///
    /// Not [`dom::Document::hears`], which reads the observing *definitions*
    /// rather than their live elements: a UA definition that observes
    /// `scroll` would make every scroll owe an event whether or not an
    /// element of it exists. What is routed and what is queued follow the
    /// live elements, as they did before the table existed;
    /// [`MainThreadRuntime::dispatch`]'s own gate is the wider `hears`.
    fn listens(&self, name: &str) -> bool {
        self.registered(name) || self.observed.contains(name)
    }

    /// Whether any realm holds a registration for `name`, of any kind on any
    /// node: the document's listener table. No document is no registration.
    fn registered(&self, name: &str) -> bool {
        self.document
            .as_ref()
            .is_some_and(|document| document.has_listeners_for(name))
    }

    /// A realm's `listenerOpened`: `key` listens for `name` on `node` in the
    /// passes `kinds` names, replacing what it reported there before.
    ///
    /// The table's edge goes to the painter at once
    /// ([`Self::forward_listener_edges`]), after the closes freed nodes left
    /// waiting, so the painter always hears a name's edges in the order they
    /// happened. A change of whether `node` has any registration for
    /// `uiappear` or `uidisappear` is what registers it for exposure
    /// detection, or lets it go ([`Exposure::set_listens`]); nothing else
    /// tells exposure about a listener.
    fn open_listener(
        &mut self,
        node: dom::NodeId,
        name: &str,
        kinds: ListenerKinds,
        key: ListenerKey,
    ) {
        self.sync_listener_edges();
        let exposure = is_exposure_event(name);
        let document = self.document.as_mut().expect(DOCUMENT_EXISTS);
        let heard = exposure && hears_exposure(document, node);
        let edge = document.add_listener(node, name, kinds, key);
        if exposure {
            self.exposure_follows(node, heard);
        }
        self.forward_listener_edges(&[(name, edge)]);
    }

    /// A realm's `listenerClosed`: `key` no longer listens for `name` on
    /// `node`, in any pass. As [`Self::open_listener`] for the edge and for
    /// exposure.
    fn close_listener(&mut self, node: dom::NodeId, name: &str, key: ListenerKey) {
        self.sync_listener_edges();
        let exposure = is_exposure_event(name);
        let document = self.document.as_mut().expect(DOCUMENT_EXISTS);
        let heard = exposure && hears_exposure(document, node);
        let edge = document.remove_listener(node, name, key);
        if exposure {
            self.exposure_follows(node, heard);
        }
        self.forward_listener_edges(&[(name, edge)]);
    }

    /// Tells exposure whether `node` has a `uiappear`/`uidisappear`
    /// registration now, when that answer is no longer `before`.
    fn exposure_follows(&mut self, node: dom::NodeId, before: bool) {
        let Self {
            document, exposure, ..
        } = self;
        let document = document.as_mut().expect(DOCUMENT_EXISTS);
        let now = hears_exposure(document, node);
        if now != before {
            exposure.set_listens(document, node, now);
        }
    }

    /// The realm `key` is closing or has been freed: every registration it
    /// filed leaves the table, each name that leaves unheard because of it
    /// closes on the painter, and it has no root any more, so no later
    /// dispatch calls it. Idempotent: a child that closed itself is
    /// forgotten again, for nothing, when it is freed.
    ///
    /// The exposure registrations of its elements are not touched: they go
    /// with the elements, which the frame's `resetFrame` frees.
    fn forget_realm(&mut self, key: ListenerKey) {
        self.roots.remove(&key);
        self.sync_listener_edges();
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let closed = document.remove_listeners_of(key);
        let edges: SmallVec<[(&str, ListenerEdge); 4]> = closed
            .iter()
            .map(|name| (&**name, ListenerEdge::Closed))
            .collect();
        self.forward_listener_edges(&edges);
    }

    /// Applies the closes the document's free path left waiting
    /// ([`dom::Document::take_listener_edges`]): a node freed by a removal or
    /// a handle's collection takes its registrations with it, and no caller
    /// was there to hand the edge to. One length check when no freed node
    /// held a registration.
    ///
    /// Drained before every edge a member forwards, so the painter hears
    /// each name's edges in order, and before every commit is published
    /// ([`Self::sync_names`]) — the end of every entry, the epilogue's
    /// [`Self::commit_if_dirty`], included — so a close a collection made in
    /// an entry crosses before that entry's `ScriptFinished` does on the same
    /// outbox FIFO, as the realm's own close did when the realm reported it.
    fn sync_listener_edges(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let edges = document.take_listener_edges();
        if edges.is_empty() {
            return;
        }
        let edges: SmallVec<[(&str, ListenerEdge); 2]> =
            edges.iter().map(|(name, edge)| (&**name, *edge)).collect();
        self.forward_listener_edges(&edges);
    }

    /// Forwards to the painter what `edges` — changes the listener table has
    /// already made, in the order it made them — move in what
    /// [`Self::routes`] says, one edge at a time.
    ///
    /// The table is the only copy of which names are registered, so the
    /// state the painter last heard is rebuilt from it: a name with an edge
    /// not yet forwarded was registered before that edge exactly when the
    /// edge is a close. That is why every caller drains the waiting closes
    /// first ([`Self::sync_listener_edges`]): then the table and what the
    /// painter heard differ by `edges` alone.
    fn forward_listener_edges(&self, edges: &[(&str, ListenerEdge)]) {
        for (index, &(name, edge)) in edges.iter().enumerate() {
            if edge == ListenerEdge::None {
                continue;
            }
            let (pending, rest) = (&edges[index..], &edges[index + 1..]);
            self.forward_routes(
                name,
                |name| self.registered_before(pending, name),
                |name| self.registered_before(rest, name),
            );
        }
    }

    /// Whether `name` was registered with the edges in `unapplied` not yet
    /// made: the first of them that names it decides, and with none the
    /// table does.
    fn registered_before(&self, unapplied: &[(&str, ListenerEdge)], name: &str) -> bool {
        unapplied
            .iter()
            .find(|&&(pending, edge)| pending == name && edge != ListenerEdge::None)
            .map_or_else(
                || self.registered(name),
                |&(_, edge)| edge == ListenerEdge::Closed,
            )
    }

    /// Sends the painter the edges of [`Self::routes`] for `name` between a
    /// registration state `before` and one `after`, the observed names
    /// unchanged. An edge may move two of its names: `dblclick` is decided
    /// here from the `click`s the painter sends, so opening it opens `click`
    /// there too, and closing it closes `click` unless `click` itself is
    /// wanted.
    fn forward_routes(
        &self,
        name: &str,
        before: impl Fn(&str) -> bool,
        after: impl Fn(&str) -> bool,
    ) {
        let paired = (name == DOUBLE_CLICK_EVENT).then_some(CLICK_EVENT);
        for routed in std::iter::once(name).chain(paired) {
            let was = self.routes_given(routed, &before);
            let now = self.routes_given(routed, &after);
            if was != now {
                self.outbox.listener_edge(Arc::from(routed), now);
            }
        }
    }

    /// Applies the observed-name edges the document recorded since the last
    /// call, forwarding each to the painter as [`Self::change_names`] says,
    /// and then the listener table's waiting closes
    /// ([`Self::sync_listener_edges`]).
    ///
    /// Called before every commit is published ([`Self::flush`],
    /// [`Self::commit_if_dirty`]): the edges cross in the mailbox the frame
    /// does, ahead of it, so the painter routes a name from the first frame
    /// that shows the element observing it. One length check each when
    /// nothing was created or freed with a definition observing a name or a
    /// registration on it, and none before the document exists.
    fn sync_names(&mut self) {
        self.sync_listener_edges();
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let edges = document.take_observed_name_edges();
        for (name, live) in edges {
            self.change_names(name, |slot| {
                if live {
                    slot.observed.insert(name);
                } else {
                    slot.observed.remove(name);
                }
            });
        }
    }

    /// Runs `change` on [`Self::observed`] for `name`, then forwards to the
    /// painter whatever moved in what [`Self::routes`] says: the painter's
    /// set is the union of the two sides, so a name crosses open when either
    /// side first wants it and closed when neither does, and a realm
    /// registration for a name an element already observes moves nothing
    /// there. The `dblclick` pairing is [`Self::forward_routes`]'s.
    fn change_names(&mut self, name: &str, change: impl FnOnce(&mut Self)) {
        let paired = (name == DOUBLE_CLICK_EVENT).then_some(CLICK_EVENT);
        let before = (self.routes(name), paired.map(|paired| self.routes(paired)));
        change(self);
        if self.routes(name) != before.0 {
            self.outbox.listener_edge(Arc::from(name), !before.0);
        }
        if let (Some(paired), Some(was)) = (paired, before.1)
            && self.routes(paired) != was
        {
            self.outbox.listener_edge(Arc::from(paired), !was);
        }
    }

    /// Whether the painter's name set must hold `name`: a realm
    /// registration or a live component element wants it
    /// ([`Self::listens`]), or, for `click`, a `dblclick`.
    fn routes(&self, name: &str) -> bool {
        self.routes_given(name, &|name| self.registered(name))
    }

    /// [`Self::routes`] with `registered` answering which names a realm
    /// registration holds.
    fn routes_given(&self, name: &str, registered: &impl Fn(&str) -> bool) -> bool {
        let wants = |name: &str| registered(name) || self.observed.contains(name);
        wants(name) || (name == CLICK_EVENT && wants(DOUBLE_CLICK_EVENT))
    }

    /// An exposure attribute of the live element `node` was written or
    /// removed; see [`Exposure::attribute_changed`].
    fn exposure_attribute_changed(&mut self, node: dom::NodeId) {
        let Self {
            document, exposure, ..
        } = self;
        exposure.attribute_changed(document.as_mut().expect(DOCUMENT_EXISTS), node);
    }

    /// The realm's `switchExposure`; see [`Exposure::switch`].
    fn switch_exposure(&mut self, on: bool, send_event: bool) {
        let Self {
            document, exposure, ..
        } = self;
        exposure.switch(document.as_mut().expect(DOCUMENT_EXISTS), on, send_event);
    }

    /// Adopts whatever metrics an attached painter has named, and records
    /// that one has.
    ///
    /// The first call that finds a value is the binding: from then on this
    /// document lays out at the painter's size, and a flush publishes rather
    /// than parking. `borrow_and_update` marks the value seen for this
    /// receiver alone, which is why the `consume_metrics` task owns a clone
    /// of its own.
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

    /// Runs the whole pipeline and publishes the committed frame — the
    /// native half of `__FlushElementTree`.
    ///
    /// Two waits can come first, and each **parks the job it runs in**,
    /// exactly as `adoptStyleSheet` parks on its response: the engine
    /// thread's tasks go on running, no other job does, and the view's own
    /// token is the biased first arm so a release ends the wait.
    ///
    /// The first is the view's listed author sheets, which the first flush
    /// settles before anything is styled: one wait per sheet whose answer has
    /// not arrived, in listed order, each mounted as it is read. A sheet that
    /// failed to load, or that the fetcher answered with something else, is
    /// an error naming its URL, which `__FlushElementTree` throws — boot's own
    /// flush fails the boot with it, also where app code settled the sheet
    /// first and caught the error. A frame is never committed without the
    /// sheets, because a frame styled without them would be published and
    /// then restyled.
    ///
    /// The second is the binding. Before any painter has bound, the frame
    /// committed here is held rather than published, because a painter
    /// composes at its own size and cannot tell that the frame it adopted
    /// predates the metrics it named, and the job parks on the metrics watch.
    /// Waking with different metrics discards the frame and commits again,
    /// which is the resize path. The sheets come first, so their IO and the
    /// painter's construction overlap and the frame held for the binding
    /// already carries them.
    ///
    /// Only the first binding is waited for. A painter that detaches leaves
    /// the last metrics behind, so every flush after it publishes at once.
    fn flush(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<(), String> {
        // The sheets first, then the metrics: a painter that binds while a
        // sheet is still in flight is adopted before the commit below, which
        // is then already at its size rather than held and recomputed.
        self.settle_sheets(thread)?;
        self.adopt_metrics();
        self.sync_names();
        let frame = self.document_mut().commit();
        if self.bound {
            // A frame held from before the binding is older than this one, and
            // publishing it after this one would put the stale frame back.
            self.held = None;
            self.outbox.publish_frame(frame);
            self.request_wanted_images();
            return Ok(());
        }
        self.held = Some(frame);
        // Asked for before the wait rather than after it: the view's own
        // fetcher serves images whether or not a painter is attached, so
        // their IO overlaps the binding instead of starting behind it.
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
            self.outbox.publish_frame(frame);
        }
        self.request_wanted_images();
        Ok(())
    }

    /// Mounts every listed author sheet not mounted yet, in listed order,
    /// waiting inside the job for each answer that has not arrived.
    ///
    /// A sheet is taken off the front of the list before it is read. The
    /// first one that fails ends the settling: its error is thrown to the
    /// caller — `__FlushElementTree` or `adoptStyleSheet` — and kept in
    /// [`Self::failed_sheet`], so every later call throws it again rather
    /// than going on with the sheets behind it.
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

    /// Commits when anything is stale, and publishes it if a painter has
    /// bound — the epilogue of every entry into the realm, which is what
    /// makes "we do not guarantee the tree is not flushed outside
    /// `__FlushElementTree`" true.
    ///
    /// It never waits: the epilogue runs after every entry, and parking here
    /// would stop the group on any of them. So while any listed author sheet
    /// is still outstanding, or one has failed, it does nothing at all: a
    /// commit without the sheets would publish an unstyled frame, and waiting
    /// for them is the first [`Self::flush`]'s. Nothing is lost by skipping —
    /// boot's own `__FlushElementTree` is what commits the first frame, and
    /// it settles the sheets before it does, or fails the boot. What it
    /// commits before the binding is held for whichever flush or binding
    /// publishes next.
    fn commit_if_dirty(&mut self) {
        // Before the sheets' check: the names an element observes are routed
        // whether or not this entry commits.
        self.sync_names();
        if !self.sheets.is_empty() || self.failed_sheet.is_some() {
            return;
        }
        self.adopt_metrics();
        if self.document_mut().needs_render() {
            let frame = self.document_mut().commit();
            if self.bound {
                // Superseded, as in `flush`: the held frame is older.
                self.held = None;
                self.outbox.publish_frame(frame);
            } else {
                self.held = Some(frame);
            }
            // The walk asked for its images either way: the view's own
            // fetcher serves them whether or not a painter is attached.
            self.request_wanted_images();
        } else if self.bound
            && let Some(frame) = self.held.take()
        {
            // Bound at the metrics this frame was already committed for, so
            // there is nothing to recompute and it goes out as it is.
            self.outbox.publish_frame(frame);
        }
    }

    /// Asks the host for the image sources the last walk discovered.
    ///
    /// The walk that just ran is the one place that knows which sources a
    /// frame needs. Empty on every commit that met no new image, which is
    /// almost all of them.
    fn request_wanted_images(&mut self) {
        let wanted = self.document_mut().take_wanted_images();
        if !wanted.is_empty() {
            self.outbox.notify(ViewNotice::RequestImages(wanted));
        }
    }

    /// Hands the host the documents an `<svg content>` gave the document
    /// since the last drain, for it to parse and report like fetched ones.
    /// Empty before `createDocument`, and on every entry that wrote no new
    /// markup.
    fn request_documents(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let requests = document.take_document_requests();
        if !requests.is_empty() {
            self.outbox.notify(ViewNotice::RequestDocuments(requests));
        }
    }

    /// The `@font-face` rules the mounted sheets declared, once each.
    ///
    /// Empty before `createDocument`: there is no cascade yet, and the
    /// epilogue of the entry that mounts a sheet asks again.
    fn take_font_face_requests(&mut self) -> Vec<dom::FontFaceRequest> {
        self.document
            .as_mut()
            .map(LynxDocument::take_font_face_requests)
            .unwrap_or_default()
    }

    /// Notes that a subtree left the tree.
    fn note_removal(&mut self) {
        self.removals = self.removals.saturating_add(1);
    }

    /// Whether enough removals have accumulated for a collection, resetting
    /// the count when they have.
    fn take_collection_due(&mut self) -> bool {
        if self.removals < REMOVALS_PER_COLLECTION {
            return false;
        }
        self.removals = 0;
        true
    }
}

/// Runs one phase of document construction, naming it if it panics.
///
/// Without this the panic would end the view as `Panicked`, which is what
/// any host member's panic does, carrying the panic's own message and nothing
/// about which of the document's several pipelines panicked.
fn construction_phase<T>(phase: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).map_err(|payload| {
        format!(
            "creating the document panicked while {phase}: {}",
            crate::threads::panic_message(payload.as_ref())
        )
    })
}

/// The private main-thread runtime used by the engine pipeline.
///
/// **The field order is the release, and it must stay in this order.** Fields
/// drop in declaration order, and two rules fix that order.
///
/// Everything that holds an `Rc` of this realm's JavaScript context is declared
/// before `slot`, so the realm is freed — and with it every host function and
/// its own clone of that `Rc` — before the `LynxDocument` those functions could
/// name. JavaScript first, then the Rust object it named. `core` is first: the
/// realm, then the timer and future tables the realm's core members wrote to,
/// neither of which runs JavaScript when it drops.
///
/// `workers` follows the realm handles: its channels belong to this MTS
/// runtime and close when it is released. JS host functions reference it
/// weakly, so queued finalizers cannot extend the channels' lifetime. A
/// child's own workers go with the child, inside `core`, before it.
pub(crate) struct MainThreadRuntime {
    /// The realm and the members every realm has, which
    /// [`crate::realm::open_realm`] installed.
    core: RealmCore,
    /// This realm's side of the workers it created, shared with the three
    /// host functions that drive them, and where a worker failure is reported
    /// from. It is also what finds the side of a child that created a
    /// worker, whose events go to that child.
    workers: Rc<super::workers::WorkerOwner>,
    slot: Rc<RefCell<DocumentSlot>>,
    /// The newest reading of the view's timeline this side has been handed —
    /// a frame post's `now` or a vsync's, both in milliseconds off the same
    /// epoch, the view's construction.
    ///
    /// The timeline is read on the painting side, where the frame clock is, so
    /// a routed event arrives carrying its own reading and this is never used
    /// for one. What it stamps is an event this side originates — an image's
    /// `load` or `error`, which settles between frames — with the last moment
    /// the two sides agreed on, and with the time origin before the first
    /// frame. One clock, one epoch; nothing here takes a second reading of its
    /// own.
    timeline_milliseconds: f64,
    /// The screen this realm's `SystemInfo` reports, as the embedder named
    /// it. Held from construction because boot writes the three numbers into
    /// its own module source, which runs after the realm exists.
    screen: ScreenMetrics,
    /// The page configuration boot writes into its own module source as five
    /// boolean literals, held for the same reason as [`Self::screen`].
    config: PageConfig,
    /// The URL the view named its MTS entry by, which boot writes into its
    /// own module source as the specifier it imports the entry by. Held for
    /// the same reason as [`Self::screen`], and read again by
    /// [`Self::complete_entry`] and [`Self::entry_module_name`].
    entry: String,
    /// The URL [`Self::complete_entry`] names as `__Card__`, when the view
    /// named one ([`RealmStartup::card`]); `None` names the entry's response
    /// URL instead.
    card: Option<String>,
    /// The BTS entry, which boot writes into its own module source and hands
    /// to the BTS in the `initialize` message. Held for the same reason as
    /// [`Self::screen`]; `None` is a view that named none.
    background_entry: Option<String>,
    /// The last `click` that did not complete a `dblclick`: its resolved
    /// target and its timestamp. A second `click` at the same target within
    /// [`DOUBLE_CLICK_MILLISECONDS`] is a `dblclick`, and clears it.
    last_click: Option<(dom::NodeId, f64)>,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl MainThreadRuntime {
    /// Opens one view's realm through [`crate::realm::open_realm`], which
    /// installs the core every realm has, and installs this realm's own host
    /// members — the document and host module, the stylesheet members, the
    /// startup strings, the native module member and the `Worker` bindings —
    /// handing back the one channel everything this view's workers say
    /// arrives on.
    ///
    /// A realm has its `Worker` members from the moment it exists: there is no
    /// state in which it is missing them, and so no order between furnishing
    /// the realm and installing them for a caller to get wrong. Which worker
    /// this realm creates is still the realm's own business — the boot module
    /// constructs the built-in background context, during the entry evaluation
    /// this call does not make.
    ///
    /// Opening spends the startup: the strings become one-shot members read
    /// later by the boot module, the screen, the BTS entry and the page
    /// configuration are kept for the boot module's own source, and the
    /// author sheets' answers go to the document slot for the first
    /// `__FlushElementTree` to settle. Nothing here waits for any of them, and
    /// the entry is not part of it, which is what lets a realm open while its
    /// own sources are still in flight.
    pub(crate) fn new(
        js_runtime: &mut ScriptRuntime,
        ingredients: DocumentIngredients,
        // The painter's metrics, as the view's seat publishes them. Link
        // state rather than a document input: it outlives the one
        // construction the ingredients are for, and an unbound flush parks on
        // it.
        metrics: watch::Receiver<Option<Viewport>>,
        outbox: ViewOutbox,
        workers: &super::workers::WorkerFactory,
        // What `adoptStyleSheet`, a `Future.wait`, a `require` and an unbound
        // `__FlushElementTree` park on: the engine thread this realm's
        // entries are jobs of.
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
        // The view's own token: what this realm's core members park on ends
        // with the view.
        let host = outbox.host_outbox(outbox.token().clone());
        // One channel for every worker of the view, the children's included,
        // and this realm's own owner of them.
        let (view_workers, incoming) = workers.view(outbox.clone());
        let workers = view_workers.owner(None);
        let document = Rc::new(DocumentHost {
            slot: Rc::clone(&slot),
            outbox,
            thread: thread.clone(),
            workers: Rc::downgrade(&view_workers),
        });
        let (core, ()) = open_realm(
            js_runtime,
            &host,
            thread,
            None,
            ScriptSource::Main,
            // Every child gets the same document members, and `Worker`
            // members of its own.
            Some(Rc::clone(&document) as Rc<dyn ChildHostModules>),
            |engine| {
                document
                    .install_members(engine, startup_strings(&mut startup), PAGE_REALM, None)
                    .map_err(MainThreadError::into_script_error)?;
                // The transport alone: the view's modules are the BTS's, and
                // this realm has no `NativeModules`. What the transport is
                // here for is a later MTS API answering calls through it.
                crate::native_module::install(engine, &host, None)
                    .map_err(|error| context_of("installing the native module members", error))?;
                workers
                    .install(engine)
                    .map_err(|error| context_of("installing Worker", error))
            },
        )
        .map_err(|error| MainThreadError::from_engine("opening the MTS realm", error))?;
        Ok((
            Self {
                core,
                workers,
                slot,
                timeline_milliseconds: 0.0,
                last_click: None,
                screen: startup.screen,
                config,
                entry: startup.entry,
                card: startup.card,
                background_entry: startup.background_entry,
            },
            incoming,
        ))
    }

    /// The realm and the two tables its core members write to, for the
    /// driver in [`crate::realm::owner`]: its epilogue, and the module loads
    /// and future settles that epilogue spawns.
    pub(crate) fn core(&mut self) -> &mut RealmCore {
        &mut self.core
    }

    /// How many `ShadowRealm` children this realm holds, grandchildren
    /// included.
    #[cfg(test)]
    pub(crate) fn child_realms(&self) -> usize {
        self.core.children.len()
    }

    /// How many of this realm's workers are still running, which is how many
    /// `Terminate`s releasing it would send.
    #[cfg(test)]
    pub(crate) fn live_workers(&self) -> usize {
        self.workers.live_workers()
    }

    /// Hands one event of one of this view's workers to the realm whose
    /// `Worker` object it is for: this realm's `bobcat-internal`, or, for a
    /// worker a `ShadowRealm` child created, the child's own — a frame's
    /// card's background thread is its child's. A worker no child recorded
    /// is this realm's, and an event for a key no realm still holds is
    /// dropped by the `bobcat-internal` it reaches.
    ///
    /// The failure report, and the end of what the realm keeps of a worker
    /// that ended, are the creating realm's side's too, so a child's
    /// worker is reported by the source that side recorded.
    pub(crate) fn dispatch_worker_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: crate::background::WorkerKey,
        payload: crate::background::WorkerPayload,
    ) -> Result<(), MainThreadError> {
        use crate::background::WorkerPayload;
        // Looked up before `forget` below drops the route of a worker that
        // ended, whose end is still the child's to hear.
        let child_owner = self.workers.child_owner(key);
        let owner = child_owner.as_ref().unwrap_or(&self.workers);
        let failed = matches!(payload, WorkerPayload::Failed(_));
        // Read before `forget` below removes it, and for a failure alone:
        // nothing else reports it, so a message clones no source. `None`
        // for a failure is a key the script has already let go of, through
        // `terminate()` or the collection of its `Worker` object: what that
        // worker still says reaches neither a `Worker` object nor the
        // embedder.
        let source = matches!(
            payload,
            WorkerPayload::Errored(_) | WorkerPayload::Failed(_)
        )
        .then(|| owner.source_of(key))
        .flatten();
        // A worker that closed itself, or whose realm could not be built or
        // whose thread trapped, has ended: this is where the realm learns
        // it, and so where the right to tell it to stop stops being worth
        // keeping.
        if matches!(payload, WorkerPayload::Closed | WorkerPayload::Failed(_)) {
            owner.forget(key);
        }
        // `(key, kind, data, filename, lineno, colno)`: `data` is the value
        // the worker posted — primitive or structured clone — for a message,
        // and the error's own message for a failure.
        let (kind, data, location) = match payload {
            WorkerPayload::Message(data) => ("message", data, None),
            WorkerPayload::Closed => ("closed", HostValue::Undefined, None),
            WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                let kind = if failed { "failed" } else { "error" };
                let data = HostValue::String(error.message.to_string());
                let location = error.location.clone();
                // Before the `Worker` object's `error` event below, so no
                // listener of it decides whether the embedder hears of this.
                if let Some(source) = source {
                    owner.report_failure(source, failed, error);
                }
                (kind, data, location)
            }
        };
        let child = child_owner.as_ref().and_then(|owner| owner.realm());
        let location = location.as_ref();
        let key = key.get().to_string();
        let arguments = [
            HostArgument::String(&key),
            HostArgument::String(kind),
            data.as_argument(),
            HostArgument::String(location.and_then(|l| l.source.as_deref()).unwrap_or("")),
            HostArgument::Number(f64::from(location.and_then(|l| l.line).unwrap_or(0))),
            HostArgument::Number(f64::from(location.and_then(|l| l.column).unwrap_or(0))),
        ];
        let called = match child {
            None => self.core.engine.call_module_export(
                js_runtime,
                WORKER_CLASS_MODULE_SPECIFIER,
                "__BobcatDispatchWorkerEvent",
                &arguments,
            ),
            // A child that is gone took its workers with it; what one said
            // last reaches no realm.
            Some(child) => self
                .core
                .children
                .call_module_export(
                    js_runtime,
                    child,
                    WORKER_CLASS_MODULE_SPECIFIER,
                    "__BobcatDispatchWorkerEvent",
                    &arguments,
                )
                .unwrap_or(Ok(false)),
        }
        .map_err(|error| MainThreadError::from_engine("delivering a worker event", error));
        let finished = self.finish_batch(js_runtime, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    /// Adopts the painter's metrics, commits when anything is stale, and
    /// publishes once a painter has bound. Called by the page's epilogue,
    /// after every entry into the realm, and never waits.
    pub(crate) fn commit_if_dirty(&mut self) {
        self.slot.borrow_mut().commit_if_dirty();
    }

    pub(crate) fn apply_page_update(
        &mut self,
        js: &mut ScriptRuntime,
        update: &crate::link::PageUpdate,
    ) -> Result<(), MainThreadError> {
        use crate::link::PageUpdate;

        let (export, arguments): (&str, &[HostArgument<'_>]) = match update {
            PageUpdate::Reload {
                data,
                processor_name,
            } => (
                "__BobcatReload",
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
                "__BobcatUpdateData",
                &[
                    HostArgument::String(data),
                    HostArgument::String(processor_name),
                    HostArgument::Boolean(*reset),
                ],
            ),
            PageUpdate::GlobalProps(data) => {
                ("__BobcatUpdateGlobalProps", &[HostArgument::String(data)])
            }
            PageUpdate::GlobalEvent { name, arguments } => (
                "__BobcatSendGlobalEvent",
                &[HostArgument::String(name), HostArgument::String(arguments)],
            ),
        };
        let called = self
            .core
            .engine
            .call_module_export(js, RUNTIME_MODULE_SPECIFIER, export, arguments)
            .map_err(|error| MainThreadError::from_engine("updating page data", error));
        let finished = self.finish_batch(js, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    /// Moves the document's animation clock to the painter's at the start of
    /// an entry; see [`dom::Document::sync_animation_clock`].
    pub(crate) fn sync_animation_clock(&mut self, now: f64) {
        self.slot
            .borrow_mut()
            .document_mut()
            .sync_animation_clock(now);
    }

    /// The document's animation clock, for the tests that pin what an entry
    /// starts at.
    #[cfg(test)]
    pub(crate) fn animation_clock(&self) -> f64 {
        self.slot
            .borrow()
            .document
            .as_ref()
            .expect(DOCUMENT_EXISTS)
            .animation_clock()
    }

    /// Advances the animation timeline to the painting side's clock
    /// reading. Whether anything changed is the next commit's business.
    pub(crate) fn begin_frame(&mut self, now: f64) {
        // Seconds here, as the animation timeline wants them; milliseconds is
        // what an event's `timestamp` is in.
        self.timeline_milliseconds = now * 1000.0;
        let _ = self
            .slot
            .borrow_mut()
            .document_mut()
            .advance_animations(now);
    }

    /// Runs the realm's animation-frame callbacks at the painting side's
    /// vsync, through `bobcat:animation-frame`, the module every realm kind
    /// files them with.
    pub(crate) fn vsync(
        &mut self,
        js: &mut ScriptRuntime,
        milliseconds: f64,
    ) -> Result<(), MainThreadError> {
        self.timeline_milliseconds = milliseconds;
        self.core
            .engine
            .call_module_export(
                js,
                ANIMATION_FRAME_MODULE_SPECIFIER,
                "__BobcatBeginFrame",
                &[HostArgument::Number(milliseconds)],
            )
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine("delivering animation callbacks", error))
    }

    /// Hands one native module's answer to one function argument of a call
    /// this realm made back to it, through `bobcat:native-modules`.
    pub(crate) fn deliver_module_callback(
        &mut self,
        js: &mut ScriptRuntime,
        call: u64,
        index: u32,
        arguments: Option<&str>,
    ) -> Result<(), MainThreadError> {
        crate::native_module::deliver(&mut self.core.engine, js, call, index, arguments).map_err(
            |error| MainThreadError::from_engine("running a native module callback", error),
        )
    }

    /// Writes the painter's scroll offsets into the document, which is how a
    /// user scroll reaches it. An offset inside the committed encode window
    /// commits nothing; one that has used half the window's headroom toward
    /// an edge ([`dom::ScrollSlot::recenter_due`]) asks this entry's commit
    /// to bake windows re-centered on it, and one past the window already
    /// did inside `scroll_to`. The scroll-driven animations those containers
    /// drive then re-sample, which is a commit only when one moved, and an
    /// anchor-positioned box the offsets shifted out of its inset-modified
    /// containing block is laid out again to pick a position fallback
    /// ([`dom::Document::redetermine_scrolled_fallbacks`]) — also a commit
    /// only then.
    ///
    /// A post naming an older request than the container's pending one
    /// ([`dom::Document::pending_scroll_request`]) is dropped: the painter
    /// made it before it saw the request, so it would write back the offset
    /// the request replaced. One that names the pending request, or a newer
    /// one, is adopted and acknowledges it. A pending auto-scroll change
    /// ([`dom::Document::auto_scroll`]) moved no offset, so no post is
    /// dropped on its account: whichever names its serial acknowledges it.
    ///
    /// CSSOM-View's `scroll` and `scrollend` are decided here, never on the
    /// painter: an adopted post that moved its container queues one
    /// `scroll` there and records it as scrolling, and a post naming a
    /// scrolling container at rest queues a `scrollend` after it. Each is
    /// queued only while a realm listener or a component hears its name at
    /// that moment, so a page that hears neither allocates nothing; the
    /// record of who is scrolling is kept regardless, so a `scrollend`
    /// listener added mid-scroll hears the end.
    pub(crate) fn adopt_scroll_offsets(
        &mut self,
        entries: impl Iterator<Item = (dom::NodeId, crate::link::ScrollEntry)>,
    ) {
        let mut slot = self.slot.borrow_mut();
        // Asked once per take: two set lookups each, and while neither name
        // is heard the loop below reads nothing more and queues nothing.
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
        let mut moved: SmallVec<[dom::NodeId; 4]> = SmallVec::new();
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
            let applied = document.scroll_to(node, entry.offset);
            // CSSOM-View: a container whose offset this post moved fires one
            // `scroll` (the mailbox holds one entry per container, so one per
            // take), and has scrolled until it comes to rest — in this post or
            // a later one.
            if applied != before {
                if hears_scroll {
                    component_events.queue_scroll(node, SCROLL_EVENT, applied - before);
                }
                if !scrolling.contains(&node) {
                    scrolling.push(node);
                }
            }
            // A rest ends a scroll only where one happened, so a container
            // posted at rest with nothing moved since its last rest fires
            // nothing. Whether `scrollend` is heard is asked now, at the end.
            if entry.rest.is_some()
                && let Some(index) = scrolling.iter().position(|&held| held == node)
            {
                scrolling.swap_remove(index);
                if hears_end {
                    component_events.queue_scroll(node, SCROLL_END_EVENT, dom::Vector2D::zero());
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
            // A container freed mid-scroll never rests.
            scrolling.retain(|node| document.get(*node).is_some());
        }
        if recenter {
            document.note_scroll_windows_stale();
        }
        document.advance_scroll_timelines(&moved);
        // css-anchor-position-1 §6.5: an anchored box the adopted offsets
        // shifted out of its inset-modified containing block determines its
        // position fallback styles again, in this entry's commit.
        document.redetermine_scrolled_fallbacks();
    }

    /// Applies the painting side's image reports, queueing the `load`s and
    /// `error`s they settle for this entry's epilogue to dispatch.
    ///
    /// Queued rather than dispatched here for one reason only — so that both
    /// producers answer the same way. The other one is the `image` component,
    /// which settles a `src` from inside the `__SetAttribute` that wrote it
    /// and may not re-enter the realm at all.
    pub(crate) fn apply_image_events(&mut self, events: &[dom::ImageEvent]) {
        let mut slot = self.slot.borrow_mut();
        let outcomes = slot.document_mut().apply_image_events(events);
        slot.component_events.extend_images(outcomes);
    }

    /// Asks the host to parse the documents an `<svg content>` handed the
    /// document since the last drain, as one
    /// [`ViewNotice::RequestDocuments`]. Its reports come back as
    /// `ToMain::ImageEvents` and apply through [`Self::apply_image_events`]
    /// like every other report. Called by the page's epilogue before its
    /// commit; nothing on this thread parses.
    pub(crate) fn request_documents(&mut self) {
        self.slot.borrow_mut().request_documents();
    }

    /// Whether the document holds anything the last commit did not apply.
    ///
    /// After the epilogue's [`Self::commit_if_dirty`] this is `true` only
    /// when that commit was skipped — a listed author sheet still
    /// outstanding — and `false` with no document yet.
    pub(crate) fn needs_render(&self) -> bool {
        self.slot
            .borrow()
            .document
            .as_ref()
            .is_some_and(LynxDocument::needs_render)
    }

    /// Queues the `scroll` every script-facing instant scroll of this entry
    /// owes: a UI method's `scrollTo`, `scrollBy` or `scrollIntoView`, or a
    /// component's own instant scroll. Asked by the page's epilogue, so it
    /// joins the component events this entry produced; one length check
    /// when nothing scrolled. The `scrollend` follows from the painter's
    /// post of the container at rest.
    pub(crate) fn queue_instant_scroll_events(&mut self) {
        let mut slot = self.slot.borrow_mut();
        if slot
            .document
            .as_ref()
            .is_some_and(LynxDocument::has_instant_scrolls)
        {
            slot.instant_scrolls();
        }
    }

    /// Whether any producer has left a component event owing — the queue
    /// [`Self::dispatch_component_events`] drains, asked once per entry.
    ///
    /// One `is_empty`, because nearly every entry's answer is no: an entry
    /// during which no source settled and no dialog closed posts nothing. A
    /// `load` handler that writes a `src` this document has already seen
    /// settle gets its outcome at the bind, and a `close` handler can close
    /// another dialog, so a delivery entry can leave the queue non-empty for
    /// an entry of its own.
    pub(crate) fn has_component_events(&self) -> bool {
        !self.slot.borrow().component_events.is_empty()
    }

    /// Dispatches everything [`Self::apply_image_events`], the `image`,
    /// `svg` and `overlay` components, the dialog's UI methods and the
    /// scroll events ([`Self::adopt_scroll_offsets`],
    /// [`Self::queue_instant_scroll_events`]) have queued, in the order they
    /// formed, and hands each connection and disconnection the `frame`
    /// component queued to `bobcat:frame` ([`Self::frame_connection`]).
    ///
    /// Called by the entry the page posts for the batch and by nothing else:
    /// the events are tasks, not part of the entry that produced them. Each
    /// is one non-bubbling dispatch at the element it belongs to, which is
    /// web-core's shape for an image's two events
    /// (`commonEventInitConfiguration.ts`: `bubbles: false`) and HTML's for a
    /// dialog's `close` and `cancel`. A [`ComponentEvent::Detailed`] does not
    /// bubble either, as native fires `<markdown>`'s `link`, `imageTap` and
    /// `parseEnd` (`lynx/core/renderer/events/touch_event_handler.cc:388-403`,
    /// ruled 2026-10-14), but is composed, so script's `target` is the
    /// component's element. An element freed between the event forming and
    /// this call resolves to nothing, exactly as a routed input event at a
    /// freed target does.
    ///
    /// Returns what the listeners threw. A handler that throws is nonfatal,
    /// the standing every event listener has here.
    pub(crate) fn dispatch_component_events(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let events = self.slot.borrow().component_events.take();
        let timestamp = self.timeline_milliseconds;
        let mut failures = Vec::new();
        for event in &events {
            // Every one of them is W3C-shaped, with no payload of its own:
            // what a Lynx handler reads in its `detail` crosses as extras.
            // An `error` and a dialog's `Event` carry nothing, and the realm's
            // `{}` is web-core's `error` detail exactly. An `<image>`'s `load`
            // carries the bitmap's *intrinsic* size, web-core's
            // `naturalWidth`/`naturalHeight`, not the box it drew into.
            //
            // An `<svg>` is native's shape instead (ruled): its `load` carries
            // the element's border-box layout size, read here at delivery —
            // after the commit that applied the natural size, because the
            // page posts this entry only once nothing is left uncommitted —
            // and 0x0 when the element has no box. It has no `error` at all.
            //
            // A `scroll` or `scrollend` carries Lynx's scroll metrics (native's
            // `bindscroll` detail, web-core's `lynxscroll`): the offset and the
            // scrolling area read here, the delta its post moved the container
            // by (0 for a `scrollend`), and `isDragging`, which this step
            // reports false, as web-core does.
            //
            // A detailed event carries the strings its component read when it
            // queued it, as extras borrowed from the queue for the call, one
            // per key the component had a value for.
            let mut extras: SmallVec<[(&'static str, Extra<'_>); 7]> = SmallVec::new();
            let (target, name, composed) = match *event {
                ComponentEvent::Image(dom::ImageOutcome::Loaded {
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
                    extras.push(("width", Extra::Number(width)));
                    extras.push(("height", Extra::Number(height)));
                    (node, LOAD_EVENT, false)
                }
                ComponentEvent::Image(dom::ImageOutcome::Failed { node }) => {
                    if is_svg(self.slot.borrow_mut().document_mut(), node) {
                        continue;
                    }
                    (node, ERROR_EVENT, false)
                }
                ComponentEvent::Plain { node, name } => (node, name, false),
                ComponentEvent::Frame { node, connected } => {
                    if let Err(error) = self.frame_connection(js_runtime, node, connected) {
                        failures.push(error);
                    }
                    continue;
                }
                ComponentEvent::Scroll { node, name, delta } => {
                    let scroll_box = self.slot.borrow_mut().document_mut().scroll_box(node);
                    let (offset, size) = scroll_box.map_or_else(
                        || (dom::Vector2D::zero(), dom::Size2D::zero()),
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
                        extras.push((key, Extra::Number(f64::from(value))));
                    }
                    extras.push(("isDragging", Extra::Bool(false)));
                    (node, name, false)
                }
                ComponentEvent::Detailed {
                    node,
                    name,
                    ref detail,
                } => {
                    push_detail_strings(detail, &mut extras);
                    (node, name, true)
                }
            };
            // None bubbles. Every one but a detailed event is not composed
            // either, as HTML and web-core fire them: one queued at a UA
            // shadow tree's node stays inside that tree.
            if let Err(error) = self.dispatch(
                js_runtime,
                target,
                name,
                false,
                composed,
                timestamp,
                &EventDetail::None,
                &extras,
            ) {
                failures.extend(error.into_errors());
            }
        }
        failures
    }

    /// Hands one `frame`'s connection (`connected`) or disconnection to
    /// `bobcat:frame` in the page realm: `__BobcatFrameConnected(frameId)`,
    /// which loads the card the frame's `src` names if none is loaded, as
    /// web-core's `LynxView` renders on `connectedCallback`, or
    /// `__BobcatFrameDisconnected(frameId)`, which disposes of it, as
    /// `disconnectedCallback` does. What happens is the module's to decide:
    /// a frame whose handle is another realm's, or that never had a `src`,
    /// is nothing to it.
    ///
    /// After a disconnection, the children nothing needs any more are freed
    /// here, outside the call, so the child the disposal retired goes in
    /// this entry rather than waiting for its epilogue.
    fn frame_connection(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        frame: dom::NodeId,
        connected: bool,
    ) -> Result<(), MainThreadError> {
        let export = if connected {
            FRAME_CONNECTED_EXPORT
        } else {
            FRAME_DISCONNECTED_EXPORT
        };
        let result = self
            .core
            .engine
            .call_module_export(
                js_runtime,
                FRAME_MODULE_SPECIFIER,
                export,
                &[HostArgument::Number(packed_node_id(frame))],
            )
            .map(drop)
            .map_err(|error| MainThreadError::from_engine("delivering a frame connection", error));
        let finished = self.finish_batch(js_runtime, result.is_ok());
        if !connected {
            self.core.children.release_unneeded();
        }
        result.and(finished)
    }

    /// Runs `probe` against the realm's document — the observation seam for
    /// everything outside this thread.
    pub(crate) fn with_document<T>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> T) -> T {
        let mut slot = self.slot.borrow_mut();
        probe(slot.document_mut())
    }

    /// Delivers one routed event the painting side decided: the type and
    /// the target crossed as plain data; the propagation path is computed
    /// here, where the document is, and the dispatch over it is the realm's.
    ///
    /// Every routed input event bubbles — the painting side routes what a
    /// gesture produced, and Lynx has no non-bubbling input event — and is
    /// composed, as every UI event is, so this is [`Self::dispatch`] with
    /// both flags set and the payload's detail. The events that do neither
    /// are the components' — an `<image>`'s `load` and `error`, a
    /// `<dialog>`'s `cancel` and `close` — which
    /// [`Self::dispatch_component_events`] delivers.
    ///
    /// A `click` names the node its release routed to as well (`release`),
    /// and is [`Self::dispatch_click`].
    pub(crate) fn dispatch_input_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        payload: &InputEventPayload,
        release: Option<dom::NodeId>,
    ) -> Result<bool, ListenerFailures> {
        if let Some(release) = release {
            return self.dispatch_click(js_runtime, target, release, payload);
        }
        // The component whose UA shadow tree a tap hit hears it first, with
        // the shadow node it hit as the target.
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
        )
    }

    /// One `click` and, when it completes one, a `dblclick` (UI Events).
    ///
    /// The target is the nearest common inclusive ancestor of the down's
    /// target and the release's, along the parents a composed event path
    /// takes ([`dom::Document::nearest_common_event_ancestor`]); the painting
    /// side has no tree to find it in. A second `click` at the same target
    /// within [`DOUBLE_CLICK_MILLISECONDS`] of the last one is then followed
    /// by a `dblclick` there, and a third starts over. The painting side sends
    /// a `click` when its name set holds `click`, which a realm `dblclick`
    /// registration opens as well ([`DocumentSlot::listener_edge`]), so each
    /// is dispatched here only when a realm listener or a component hears
    /// it.
    fn dispatch_click(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        down: dom::NodeId,
        release: dom::NodeId,
        payload: &InputEventPayload,
    ) -> Result<bool, ListenerFailures> {
        let (target, click, double) = {
            let slot = self.slot.borrow();
            let Some(document) = slot.document.as_ref() else {
                return Ok(false);
            };
            (
                document.nearest_common_event_ancestor(down, release),
                slot.listens(CLICK_EVENT),
                slot.listens(DOUBLE_CLICK_EVENT),
            )
        };
        let Some(target) = target else {
            self.last_click = None;
            return Ok(false);
        };
        let at = payload.timestamp;
        let clicked = if click {
            self.dispatch(
                js_runtime,
                target,
                CLICK_EVENT,
                true,
                true,
                at,
                &payload.detail,
                &[],
            )
        } else {
            Ok(false)
        };
        let completes = self
            .last_click
            .take_if(|(last, then)| *last == target && at - *then <= DOUBLE_CLICK_MILLISECONDS)
            .is_some();
        if !completes {
            self.last_click = Some((target, at));
            return clicked;
        }
        let doubled = if double {
            self.dispatch(
                js_runtime,
                target,
                DOUBLE_CLICK_EVENT,
                true,
                true,
                at,
                &payload.detail,
                &[],
            )
        } else {
            Ok(false)
        };
        // What a `click` listener threw does not keep the `dblclick` from
        // running, and both are answered, the `click`'s first.
        match (clicked, doubled) {
            (Ok(clicked), Ok(doubled)) => Ok(clicked || doubled),
            (Err(failed), Ok(_)) | (Ok(_), Err(failed)) => Err(failed),
            (Err(mut clicked), Err(doubled)) => {
                clicked.0.extend(doubled.0);
                Err(clicked)
            }
        }
    }

    /// One whole dispatch, whatever produced it: the one place an event is
    /// delivered. The path is computed once, and one walk runs over it —
    /// the engine components' handlers at each step, and at the step of each
    /// listening realm's root one call into that realm, which runs the
    /// realm's own Lynx walk over its part of the path.
    ///
    /// A target freed since the event formed resolves to nothing rather than
    /// a path — a `NodeId` names one node for the life of the document, so
    /// the check is one lookup and can never hit a stranger — and so does a
    /// name nothing hears ([`dom::Document::hears`]: no registration and no
    /// observing definition), which costs that one check and builds no path.
    ///
    /// **Which realms are called** is decided before the walk, from the
    /// document's listener table, and is not revisited during it
    /// (`docs/tracking/deviations.md`): a realm is called when it holds a
    /// path registration for the name on a node of the path, in a pass that
    /// node's steps run ([`dom::Document::listeners_on_path`]); when it holds
    /// a `global-bindEvent` registration for the name anywhere
    /// ([`dom::Document::realm_has_global`]); or, for a document-level name
    /// ([`DOCUMENT_LEVEL_EVENTS`]), when it holds any registration for it.
    /// A realm with nothing to run is not called, so an event nothing on its
    /// path listens for costs no crossing at all.
    ///
    /// **Where each is called** is the capture step of its root. The page
    /// realm's page is the document element, and it is called at the first
    /// step of the path — the document node above that element for every
    /// path in the document tree, so it is handed the whole path, exactly
    /// what the one call before the listener table carried; or the root of a
    /// detached tree's path, which is the page realm's too. A `frame`'s
    /// card's root is the nested page its `framePage` answered
    /// ([`DocumentSlot::roots`]). A document-level name calls the children at
    /// the document element, after the page realm, in the order they were
    /// constructed. A realm whose root is not
    /// on the path is not called: a card's `global-bindtap` hears the taps
    /// inside its frame and no other. The walk is [`dom::Document::advance`],
    /// one step at a time with the document borrowed for that step alone, so
    /// a realm's listeners may mutate the tree; a component's
    /// `stop_propagation` ends the walk, the realm calls after it included.
    /// A realm's own `catch` and `stopPropagation` end that realm's walk and
    /// nothing else, as web-core's do (user ruling, 2026-10-18).
    ///
    /// **Each call** hands the realm its part of the path as the standard's
    /// event path: the capture steps from its root inward, in target-first,
    /// root-last order, each with its shadow-retargeted target, keeping the
    /// steps of the root's own tree and no other — no shadow root, no UA
    /// component's shadow content, no nested frame's card — encoded as two
    /// comma-joined decimal id strings, which is how `childElementIds`
    /// carries a list already: the boundary takes primitives and structured
    /// clones only, a clone can be minted by the realm alone, and a decimal
    /// id cannot contain the separator. For the page realm that is the whole
    /// path less every step inside a shadow tree, which is exactly what the
    /// one call before the listener table carried: script names no node of a
    /// UA shadow tree — no handle, no listener — and a browser reports
    /// exactly that to every listener outside the tree: the host,
    /// retargeted, as the first step that remains says. An event at shadow
    /// content therefore starts at its host there, and a non-composed one
    /// there reaches no realm at all. The components' walk keeps those steps:
    /// a component's shadow tree is its own. A realm called at a step that is
    /// not of its tree (a card hearing a key at the document element) is
    /// handed no step, and runs its `global-bindEvent` pass alone.
    ///
    /// `bubbles` crosses as itself rather than as a shortened path, because
    /// the passes narrow differently: a non-bubbling event still captures down
    /// the whole path, binds on its at-target steps alone, and runs no
    /// `global-bindEvent` pass at all — web-core's `common_event_handler`
    /// (`web-core/src/main_thread/client/element_apis/event_apis.rs:413-432`),
    /// which is handed the same one flag off the DOM event
    /// (`WASMJSBinding.ts:254-258`). Sending a narrowed path instead would
    /// lose the capture pass.
    ///
    /// `composed` is the event's own flag too, and it shapes the path here,
    /// where the tree is: a non-composed event's path ends at the root of the
    /// tree its target is in (DOM's *get the parent*).
    ///
    /// Everything else crosses as primitives, and the realm builds the
    /// objects, in this order: the `timestamp`; the extras channel
    /// ([`encode_extras`]: the count, then name/value pairs), which is what a
    /// Lynx handler reads beside the W3C payload and which the realm makes
    /// the Lynx `detail` whenever there are any; then the W3C payload as
    /// [`encode_detail`] writes it, last because a touch event's points give
    /// it no fixed length. The kernel's events are W3C-shaped, and the realm
    /// is the Lynx layer that reshapes them (user ruling, 2026-10-13). No
    /// JSON is formatted here and none is parsed there.
    ///
    /// Each call is its own module-export call with its own microtask
    /// checkpoint, as a browser checkpoints after each listener invocation.
    /// A call that throws is kept and the walk goes on, so every realm still
    /// hears the event; [`Self::finish_batch`] runs once, at the end; and
    /// every failure is answered, in the order the calls ran.
    ///
    /// Answers whether some realm published the export, which is all the host
    /// can know: nothing here says whether anything ran.
    #[allow(
        clippy::too_many_arguments,
        reason = "the event's two flags, as `dom::Document::event_steps` takes them"
    )]
    fn dispatch(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        bubbles: bool,
        composed: bool,
        timestamp: f64,
        detail: &EventDetail,
        extras: Extras<'_>,
    ) -> Result<bool, ListenerFailures> {
        // The realm files every name lowercased — `element-papi.ts`
        // lowercases both halves of a registration, and its dispatch the
        // name it is handed — so the listener table is asked in that
        // spelling; a component's own name (`parseEnd`) keeps its case for
        // the components' walk. One scan, and no allocation for a name that
        // is lowercase already, which every routed input name is.
        let lowered;
        let registered = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
            lowered = name.to_ascii_lowercase();
            lowered.as_str()
        } else {
            name
        };
        let children = self.core.children.keys();
        let (steps, calls) = {
            let slot = self.slot.borrow();
            let document = slot.document.as_ref().expect(DOCUMENT_EXISTS);
            if document.get(target).is_none()
                || !(document.hears(registered) || document.observes_event(name))
            {
                return Ok(false);
            }
            let steps = document.event_steps(target, bubbles, composed);
            let calls = realm_calls(&slot, document, &steps, registered, &children);
            (steps, calls)
        };

        let mut walk = self
            .slot
            .borrow()
            .document
            .as_ref()
            .expect(DOCUMENT_EXISTS)
            .event_walk(&steps, name, detail, timestamp);
        let mut failures: SmallVec<[MainThreadError; 1]> = SmallVec::new();
        let mut called = false;
        loop {
            // The document is borrowed for one step's components alone: a
            // realm call below runs listeners that mutate the tree.
            let outcome = self.slot.borrow_mut().document_mut().advance(&mut walk);
            let Some(outcome) = outcome else {
                break;
            };
            if !outcome.capture {
                continue;
            }
            for call in calls.iter().filter(|call| call.at == outcome.node) {
                match self.call_realm(js_runtime, call, name, bubbles, timestamp, detail, extras) {
                    Ok(published) => called |= published,
                    Err(error) => failures.push(error),
                }
            }
        }
        // Listeners remove elements too; the count they ran up is settled
        // here, at the end of the dispatch, as long as nothing failed.
        if let Err(error) = self.finish_batch(js_runtime, failures.is_empty()) {
            failures.push(error);
        }
        ListenerFailures::or_called(failures, called)
    }

    /// One realm's part of one dispatch: `__BobcatDispatchEvent(nodes,
    /// targets, name, bubbles, timestamp, extras…, detail…)` in the page
    /// realm, or in the child `call` names. A child that closed since the
    /// walk began — a listener before it let go of its frame — has no root
    /// any more and is not called, nor is one already freed.
    #[allow(
        clippy::too_many_arguments,
        reason = "the dispatch's own arguments, which every call carries"
    )]
    fn call_realm(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        call: &RealmCall,
        name: &str,
        bubbles: bool,
        timestamp: f64,
        detail: &EventDetail,
        extras: Extras<'_>,
    ) -> Result<bool, MainThreadError> {
        // The detail last, because a touch event's points make it the one
        // part of variable length besides the extras before it.
        let mut arguments = Arguments::new();
        arguments.push(HostArgument::String(&call.nodes));
        arguments.push(HostArgument::String(&call.targets));
        arguments.push(HostArgument::String(name));
        arguments.push(HostArgument::Boolean(bubbles));
        arguments.push(HostArgument::Number(timestamp));
        encode_extras(extras, &mut arguments);
        encode_detail(detail, &mut arguments);

        let called = match call.child {
            None => self.core.engine.call_module_export(
                js_runtime,
                ELEMENT_MODULE_SPECIFIER,
                EVENT_DISPATCH_EXPORT,
                &arguments,
            ),
            Some(child) => {
                if !self.slot.borrow().roots.contains_key(&call.key) {
                    return Ok(false);
                }
                let Some(called) = self.core.children.call_module_export(
                    js_runtime,
                    child,
                    ELEMENT_MODULE_SPECIFIER,
                    EVENT_DISPATCH_EXPORT,
                    &arguments,
                ) else {
                    return Ok(false);
                };
                called
            }
        };
        called.map_err(|error| MainThreadError::from_engine("delivering an event", error))
    }

    /// Fires a non-composed `tap` at `target` for the engine's own
    /// components, when `target` is UA shadow content: the component whose
    /// shadow tree it is hears which of its inner nodes was tapped, as
    /// web-core's components listen for `click` on their own shadow content
    /// (`XMarkdownAttributes.ts`, `#handleClick`).
    /// [`Self::dispatch_input_event`] calls it before the one dispatch of the
    /// same `tap`.
    ///
    /// That dispatch's Rust walk cannot do this: the routed `tap` is
    /// composed, so the host is on its path as itself and hears it with the
    /// target retargeted to the host — which inner node was hit is not on
    /// the event any more. Bubbling and **not composed**, this walk ends at
    /// the shadow root instead, whose step the host's `handle_event` hears
    /// with the inner node as `event.target()` — once in the capture pass and
    /// once in the bubble pass, as `slotchange` is heard. A component that
    /// observes `tap` therefore hears three deliveries of one tap: these two,
    /// and the composed one at the host, [`dom::event::EventPhase::AtTarget`]
    /// with `target()` the host itself, which a handler that wants the inner
    /// node ignores. A defined element inside the same shadow tree, on the
    /// path, hears it as well. A host hears nothing of a tap at its light-DOM
    /// content, which it only displays, and an outer host nothing of one
    /// inside a nested component's shadow tree.
    ///
    /// Nothing about it reaches script, and a handler that stops propagation
    /// stops this walk alone. The detail and the timestamp are the routed
    /// event's own.
    ///
    /// A type nothing observes returns after one set lookup, and a target
    /// outside every shadow tree after one walk up its ancestors, before any
    /// path is built, which is the whole cost of a tap at ordinary content.
    fn tap_shadow_content(&mut self, target: dom::NodeId, detail: &EventDetail, timestamp: f64) {
        let mut slot = self.slot.borrow_mut();
        let document = slot.document_mut();
        if !document.observes_event(TAP_EVENT) || document.shadow_root_of(target).is_none() {
            return;
        }
        document.dispatch_element_event(target, TAP_EVENT, detail, true, false, timestamp);
    }

    /// [`Self::dispatch_input_event`] for a test that is about the dispatch
    /// rather than about the payload: the event happened at `position`, with
    /// no wheel delta, no touch points and the time origin for a timestamp.
    #[cfg(test)]
    pub(crate) fn dispatch_for_test(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        position: dom::Point2D<f32>,
    ) -> Result<bool, ListenerFailures> {
        self.dispatch_input_event(
            js_runtime,
            target,
            name,
            &InputEventPayload {
                detail: dom::event::EventDetail::Pointer {
                    position,
                    id: 1,
                    device: dom::input::PointerKind::Touch,
                    button: 0,
                    buttons: 0,
                },
                ..InputEventPayload::default()
            },
            None,
        )
    }

    /// Whether the commits since the last delivery left any
    /// `content-visibility: auto` skipping change owing — css-contain-2
    /// §4.4's queue, asked once per entry.
    ///
    /// One `is_empty` on a `Vec`, because nearly every entry's answer is no:
    /// a page whose `auto` boxes did not change state posts nothing.
    pub(crate) fn has_pending_content_visibility_changes(&self) -> bool {
        self.slot
            .borrow()
            .document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_content_visibility_changes)
    }

    /// Fires one `contentvisibilityautostatechange`
    /// ([css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event))
    /// per queued change, in frame order.
    ///
    /// The whole dispatch is `dom`'s: it owns the path, the walk and the
    /// listeners, because the listeners are the engine's own components
    /// (`dom::CustomElement` definitions — `<image>` today, `<list>` when it
    /// exists) rather than anything in a realm. **No realm is entered**,
    /// which is why this takes no `ScriptRuntime`: the event has no script
    /// form, no Lynx event name, and nothing about it is published to the
    /// painting or the background side. What this method adds is *when*: the
    /// spec dispatches the event "by posting a task at the time when the
    /// state change occurs", and the entry the page posts for it is that
    /// task.
    ///
    /// A handler may mutate the tree — the changes are taken before the walk
    /// starts, each step re-checks its node, and whatever a handler wrote is
    /// committed by this entry's own epilogue.
    pub(crate) fn dispatch_content_visibility_changes(&mut self) {
        let timestamp = self.timeline_milliseconds;
        self.slot
            .borrow_mut()
            .document_mut()
            .dispatch_content_visibility_changes(timestamp);
    }

    /// Runs the W3C "update intersection observations" steps
    /// ([§3.2.10](https://w3c.github.io/IntersectionObserver/#update-intersection-observations-algo))
    /// over the document's observers, and answers whether any of them has
    /// entries queued afterwards — the page's cue to post a delivery.
    ///
    /// Asked once per entry, after its commit, and nearly always answered at
    /// once: `dom` runs nothing when there are no observers or nothing went
    /// stale since the last update that ran — a frame built, an offset
    /// moved, a target observed. It reads the last completed layout and the
    /// live scroll offsets and runs no pass, so an entry whose scroll was
    /// composed inside the encode window, and committed nothing, still
    /// updates against the offset it adopted.
    ///
    /// Every entry it queues is stamped with [`Self::timeline_milliseconds`]:
    /// the reading the latest `vsync` or frame post handed this side, which
    /// is the `DOMHighResTimeStamp` that frame's animation-frame callbacks
    /// were called with and the one an image's `load` is stamped with. That
    /// is HTML's order — the observations are updated in the same rendering
    /// step as the callbacks, after them — so the entries' `time` compares
    /// directly with every other time this side reports, and nothing takes a
    /// second reading of the clock.
    pub(crate) fn update_intersection_observations(&mut self) -> bool {
        let time = self.timeline_milliseconds;
        self.slot
            .borrow_mut()
            .document
            .as_mut()
            .is_some_and(|document| document.update_intersection_observations(time))
    }

    /// Whether exposure transitions wait for a delivery entry — the page's
    /// cue to post one even when no observer has entries queued: a
    /// teardown, a `stopExposure` that sent records, or a freed element that
    /// was exposed queues its transition outside any update.
    ///
    /// One `is_empty`, because nearly every entry's answer is no.
    pub(crate) fn has_pending_exposure(&self) -> bool {
        self.slot.borrow().exposure.has_transitions()
    }

    /// Delivers every observer's queued entries to its handler, in creation
    /// order — "notify intersection observers"
    /// ([§3.2.5](https://w3c.github.io/IntersectionObserver/#notify-intersection-observers-algo)),
    /// which `dom` runs whole, call included — and then the exposure
    /// transitions that loop and the entries before it left, into the realm.
    ///
    /// Two kinds of observer exist today. An engine component's, a
    /// `dom::CustomElement` definition's, carries a [`dom::ElementHandler`]:
    /// `dom` calls that element's `intersections_changed` hook in its own
    /// `[CEReactions]` scope, and drops the entries if the element is not a
    /// constructed component; **no realm is entered** for one. An exposure
    /// registration's carries the exposure handler, which only records
    /// transitions: it cannot call the realm from inside `dom`'s loop, because
    /// the realm is not the document's to lend, so the realm half runs here,
    /// after the loop, with the document released. The MTS
    /// `IntersectionObserver` binding's handler, once it exists, is the same
    /// shape: it queues `(observer, entries)` and this entry drains them after
    /// the loop.
    ///
    /// That half is web-core's order (`ExposureServices.ts:209-275`): each
    /// `uiappear`/`uidisappear` in the order the transitions formed — one
    /// non-bubbling, non-composed dispatch at the element, with
    /// [`EventDetail::Exposure`] — then one `exposure` record list, then one
    /// `disexposure` list, each one call of `bobcat:runtime`'s
    /// `__BobcatSendExposure` (see [`Self::send_exposure_records`]). An
    /// element freed since its transition formed hears nothing, as a routed
    /// event at a freed target does, and its record is still sent: the id
    /// and scene were copied when it formed.
    ///
    /// A hook or a listener may mutate the tree. Whatever it wrote is
    /// committed by the delivery entry's own epilogue, whose update then runs
    /// again against the new frame.
    ///
    /// Returns what failed: a listener that threw, or a record list the realm
    /// did not take. Each is nonfatal and reported as `ListenerFailed`; the
    /// rest is still delivered.
    pub(crate) fn notify_intersection_observers(
        &mut self,
        js: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        self.slot
            .borrow_mut()
            .document_mut()
            .notify_intersection_observers();
        let transitions = self.slot.borrow().exposure.take_transitions();
        if transitions.is_empty() {
            return Vec::new();
        }
        let timestamp = self.timeline_milliseconds;
        let mut failures = Vec::new();
        for transition in transitions.iter().filter(|t| t.element_event) {
            let name = if transition.appeared {
                APPEAR_EVENT
            } else {
                DISAPPEAR_EVENT
            };
            // web-core's `bubbles: false, composed: false`
            // (`ExposureServices.ts:210-218`).
            if let Err(error) = self.dispatch(
                js,
                transition.node,
                name,
                false,
                false,
                timestamp,
                &EventDetail::Exposure,
                &[],
            ) {
                failures.extend(error.into_errors());
            }
        }
        for (kind, appeared) in [("exposure", true), ("disexposure", false)] {
            let mut record = String::new();
            for transition in transitions.iter().filter(|t| t.appeared == appeared) {
                let Some(id) = &transition.exposure_id else {
                    continue;
                };
                write_record_field(&mut record, &packed_node_id(transition.node).to_string());
                write_record_field(&mut record, id);
                write_record_field(&mut record, &transition.scene);
            }
            if record.is_empty() {
                continue;
            }
            if let Err(error) = self.send_exposure_records(js, kind, &record) {
                failures.push(error);
            }
        }
        failures
    }

    /// Hands one global record list to the realm: `bobcat:runtime`'s
    /// `__BobcatSendExposure(kind, record)`, `kind` being `"exposure"` or
    /// `"disexposure"` and `record` a `bobcat:record` payload of three fields
    /// per element — its packed node id, its `exposure-id`, its scene. The
    /// realm builds web-core's record objects out of them and sends the list
    /// to the BTS as the global event of that name; nothing about their shape
    /// is decided here.
    ///
    /// A realm that publishes no such export is a failure rather than a
    /// silent drop, as it is not for the exports a bundle may lack: this one
    /// is the engine's own, and records that reach nobody are lost.
    fn send_exposure_records(
        &mut self,
        js: &mut ScriptRuntime,
        kind: &str,
        record: &str,
    ) -> Result<(), MainThreadError> {
        const CONTEXT: &str = "delivering exposure records";
        let called = self
            .core
            .engine
            .call_module_export(
                js,
                RUNTIME_MODULE_SPECIFIER,
                EXPOSURE_EXPORT,
                &[HostArgument::String(kind), HostArgument::String(record)],
            )
            .map_err(|error| MainThreadError::from_engine(CONTEXT, error));
        let finished = self.finish_batch(js, called.is_ok());
        called
            .and_then(|published| {
                if published {
                    Ok(())
                } else {
                    Err(MainThreadError::from_engine(
                        CONTEXT,
                        ScriptError {
                            kind: crate::script::ScriptErrorKind::Other,
                            phase: crate::script::ScriptErrorPhase::CallModuleExport,
                            message: format!(
                                "{RUNTIME_MODULE_SPECIFIER} publishes no {EXPOSURE_EXPORT}"
                            )
                            .into(),
                            location: None,
                        },
                    ))
                }
            })
            .and(finished)
    }

    /// When the earliest armed timer comes due, if one is armed, for a test
    /// that drives this realm's timers itself. The page's epilogue reads the
    /// same table through the realm's core.
    #[cfg(test)]
    pub(crate) fn next_timer_deadline(&mut self) -> Option<ClockInstant> {
        self.core.timers.next_deadline()
    }

    /// Runs every timer due now, in the order the standard fires them: this
    /// realm's, then each `ShadowRealm` child's, in construction order.
    ///
    /// Returns whatever their callbacks threw. A timer is its own task, so
    /// one that throws neither stops the ones behind it nor ends the realm —
    /// the same standing an event listener that throws already has. A
    /// child's callbacks are in the same batch, because what they call of
    /// this realm's can remove elements too.
    ///
    /// The `x-swiper` autoplay turns that came due run first. They are the
    /// component's, not script's: each asks the painter for one scroll and
    /// calls into no realm, so they throw nothing, and what they ask for
    /// rides this entry's commit like a timer callback's mutation.
    pub(crate) fn run_due_timers(&mut self, js_runtime: &mut ScriptRuntime) -> Vec<ScriptError> {
        self.slot.borrow_mut().run_due_autoplays();
        let own = run_due_timers(&mut self.core.engine, js_runtime, &self.core.timers);
        let children = self.core.children.run_due_timers(js_runtime);
        if own.is_none() && children.is_none() {
            return Vec::new();
        }
        let mut failures: Vec<ScriptError> = own.into_iter().chain(children).flatten().collect();
        // Callbacks remove elements like any other realm entry point; the
        // count they ran up is settled here, at the end of the batch.
        if let Err(error) = self.finish_batch(js_runtime, true) {
            failures.push(error.into_script_error());
        }
        failures
    }

    /// The earliest deadline this page waits for: its realm's next timer, or
    /// an `x-swiper`'s next autoplay turn, whichever comes first.
    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        let timers = self.core.timers.next_deadline();
        let autoplays = self.slot.borrow().autoplays.next_deadline();
        match (timers, autoplays) {
            (Some(timers), Some(autoplays)) => Some(timers.min(autoplays)),
            (timers, autoplays) => timers.or(autoplays),
        }
    }

    /// Evaluates `bobcat:boot`, which is this realm's whole startup: it
    /// creates the document, imports the entry, connects the BTS Worker,
    /// renders, and flushes.
    ///
    /// Nothing is waited for before this runs. The entry reaches the realm
    /// through an ordinary `import` of the URL the view named it by, a module
    /// a task of the view's owner completes from the answer
    /// `create_lynx_view` already asked for, with nothing added to it: a
    /// card's MTS body already carries the imports `bobcat-source` prepended
    /// when it registered it (see [`Self::complete_entry`]). The author
    /// stylesheets are mounted by boot's own `__FlushElementTree`, which waits
    /// for each before the document is styled. So this returns with a
    /// document in place however long the entry takes, and boot's own
    /// completion is the promise `main_module_finished` reads. The only two
    /// things boot waits on are both inside that flush: the listed sheets,
    /// and a painter's binding.
    ///
    /// How much of boot has run when this returns depends on the entry alone:
    /// an entry already completed is found in the realm's registry and boot
    /// runs through to its own flush here; one still outstanding leaves the
    /// import pending, and the rest of boot runs in the job that completes it.
    ///
    /// **The entry is the app's code, and the rest is the engine's.** Boot
    /// catches what the entry's import throws — the entry's evaluation, or a
    /// module it imports — and raises it again as a rejection nothing
    /// handles, so the checkpoint of the entry that ran the catch reports it,
    /// once, and boot goes on to connect the BTS, render and flush. What
    /// fails boot's evaluation is therefore the engine's own code alone:
    /// `bobcat:runtime` reading the page data, the document's construction,
    /// connecting the BTS, and the flush, the listed sheets included.
    ///
    /// The only literals written into it are the screen's three numbers, the
    /// page configuration's five switches, the entry's URL and the BTS
    /// entry's — facts Rust owns, written as primitives rather than as JSON
    /// the realm would parse and hand back. Each URL is written as a JSON
    /// string literal, which is the one quoting that is also a JavaScript
    /// string literal, and a view that named no BTS entry writes `undefined`.
    /// `SystemInfo` describes the screen the page is shown on, which this
    /// view's viewport is not; the switches are what the realm builds its
    /// document with and what `enableJSDataProcessor` tells the MTS runtime.
    /// The BTS entry is not this realm's to import: boot hands it to
    /// `__BobcatConnectBackground`, which posts it to the BTS in the
    /// `initialize` message beside this realm's `SystemInfo` and the view's
    /// native module table.
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
try {{ await import({entry}); }} catch (error) {{ void Promise.reject(error); }}
const {{ Worker }} = await import("bobcat-internal");
data = __BobcatProcessInitData(data);
// The BTS's own entry, by the URL the view named it by, is the BTS's to
// import: this call posts that URL to the BTS in the `initialize` message.
__BobcatConnectBackground(new Worker("{BTS_MODULE_SPECIFIER}", {{ name: "lynx-bg" }}), data, {background_entry});

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

    /// Boots a realm over a card's MTS body as its entry, without a fetcher —
    /// the seam this crate's own tests and benchmarks drive boot through.
    ///
    /// `source` is the body as a container carries it, and the entry is that
    /// body the way `bobcat-source` registers a card's root script:
    /// [`crate::esm::MTS_CHUNK_PREAMBLE`] and then the body, on the
    /// preamble's own line, so the body's first line is the module's first.
    /// Nothing else prepends it: [`Self::complete_entry`] completes an entry
    /// with the source it is handed.
    ///
    /// `source_name` is both the URL the entry is requested by and the one it
    /// is answered from; it replaces whatever entry the realm was opened with.
    /// The production boot runs up to its `import` of the entry, and the entry
    /// is then completed with the source in hand, as the view's own entry task
    /// would with a fetcher's answer, which runs the rest of boot: the same
    /// module, the same members and the same order a view boots in.
    ///
    /// The result is the first failure: boot's own before its import, the
    /// entry's evaluation — which a view reports without failing its boot —
    /// or boot's own after it; and `Ok` once boot settled or while it still
    /// waits for a resource or a timer. Naming the entry is the engine's own
    /// code and does not fail for an entry named by an absolute URL, which
    /// every caller of this seam uses.
    pub(crate) fn run_main_thread_script(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        source_name.clone_into(&mut self.entry);
        self.run_boot_module(js_runtime)?;
        // The one request boot left is its entry, which the view's owner
        // never sends to a fetcher; it is answered below.
        let requested = self.take_module_request();
        debug_assert_eq!(requested, self.entry_module_name().ok());
        self.complete_entry(js_runtime, source_name, &card_entry(source))
            .expect("naming an entry by an absolute URL does not fail")?;
        let finished = self.main_module_finished().map(|_| ());
        let collected = self.finish_batch(js_runtime, finished.is_ok());
        finished.and(collected)
    }

    /// Boots a realm over a card's MTS body as its entry the way a view whose
    /// author sheets the fetcher answered before its first flush boots: each
    /// sheet is handed to the document slot as an answer already in hand, in
    /// the order given, which is the listed order, and boot's own
    /// `__FlushElementTree` mounts them before it styles the document.
    /// `source` is the body, which becomes the entry the way it does in
    /// [`Self::run_main_thread_script`], and `source_name` is both the
    /// entry's request URL and its response URL, as there.
    ///
    /// The seam for this crate's own tests of cards that come with a sheet.
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
        // The one request boot left is its entry, which the view's owner
        // never sends to a fetcher.
        assert_eq!(self.take_module_request(), self.entry_module_name().ok());
        self.complete_entry(js_runtime, source_name, &card_entry(source))
            .expect("naming an entry by an absolute URL does not fail")
    }

    /// The next module an import in this realm is waiting for, if any.
    pub(crate) fn take_module_request(&mut self) -> Option<String> {
        self.core.engine.take_module_request()
    }

    /// The name boot's `import` of the entry asks the realm for: the entry's
    /// URL, normalized the way the module loader normalizes a specifier
    /// imported from `bobcat:boot`. For this entry the normalization is the
    /// identity: `create_lynx_view` already resolved it to an absolute URL's
    /// serialization, which the loader serializes to itself. A name that is
    /// not an absolute URL, which only a seam that skips `create_lynx_view`
    /// can put here, is refused with the loader's own message, which is also
    /// what rejects boot's `import`.
    pub(crate) fn entry_module_name(&self) -> Result<String, String> {
        super::quickjs::normalize_module_url(BOOT_MODULE_SPECIFIER, &self.entry)
    }

    /// Completes the module boot imports the entry as, from the script the
    /// fetcher answered the entry request `create_lynx_view` made with: its
    /// response URL and its source. An answer that is not a script never gets
    /// here — it fails the view's startup before any of it runs.
    ///
    /// The module is the one [`Self::entry_module_name`] names — the name
    /// boot's `import` asks for, the request URL, which is also the name its
    /// errors and stack frames carry — completed like any other import: with
    /// the source exactly as the fetcher answered it, from the fetcher's
    /// response URL. That URL is the base its own relative imports resolve
    /// against and its `import.meta.url`.
    ///
    /// **Nothing is added to the source.** What a card's MTS body imports —
    /// [`crate::esm::MTS_CHUNK_PREAMBLE`] — is `bobcat-source`'s to prepend,
    /// and it does so to every card body it registers, so the fetcher already
    /// answers a card's root with its imports in place. A line of the source
    /// is the line its errors report. An entry that is not a card's body
    /// imports what it uses itself, and one that names a binding it did not
    /// import throws a `ReferenceError` when that line runs, which is the
    /// entry's own failure.
    ///
    /// **The card is named here.** Before the script is completed, the
    /// view's card URL ([`ViewSources::card_url`](crate::ViewSources::card_url))
    /// — or, for a view that named none, the response URL, the fetcher's, so
    /// a redirect is already applied — is handed to `bobcat:runtime`'s
    /// `__BobcatInitEntry`, which makes it `__Card__`: this page's own
    /// container URL for the body, for every chunk and stylesheet it names by
    /// `__Card__`, and for the base URL a `new Worker` specifier resolves
    /// against. A chunk is never named: it would overwrite `__Card__` with its
    /// own URL.
    ///
    /// Two failures, one inside the other. The outer one is the engine's own
    /// code — the entry's module name, and the call that names it — and fails
    /// the view's startup. The inner one is the completion and its
    /// checkpoint, which is where what the entry's evaluation threw arrives:
    /// boot catches it and raises it again unhandled (see
    /// [`Self::run_boot_module`]), so it is the app's failure, reported
    /// without ending the view.
    ///
    /// Boot has run first: `bobcat:runtime` is one of its static imports, so
    /// it is instantiated by the time this is called, and boot's `import` of
    /// the entry is the pending request this completion resumes. The view's
    /// entry task is queued behind `open_realm`, which runs boot, and
    /// [`Self::run_main_thread_script`] and its variant with sheets keep
    /// the same order.
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
        // Without a checkpoint of its own: the completion below drains the
        // queue for both, so naming the entry wakes no sibling realm between
        // them.
        self.core
            .engine
            .call_module_export_before_operation(
                js_runtime,
                RUNTIME_MODULE_SPECIFIER,
                "__BobcatInitEntry",
                &[HostArgument::String(self.card.as_deref().unwrap_or(url))],
            )
            .map_err(starting)?;
        Ok(self
            .core
            .engine
            .complete_module(js_runtime, &name, Ok((url, source)))
            .map_err(booting))
    }

    /// The `@font-face` rules the sheets mounted so far declared and this
    /// realm has not reported before. Empty until a sheet is mounted.
    pub(crate) fn take_font_face_requests(&mut self) -> Vec<dom::FontFaceRequest> {
        self.slot.borrow_mut().take_font_face_requests()
    }

    /// Files one loaded `@font-face` source under its declared family.
    pub(crate) fn register_font_face(&mut self, family: &str, data: dom::FontBlob) {
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

    /// Runs the MTS module's asynchronous disposal through the ordinary ESM
    /// evaluator, as the view ends. JavaScript owns the BTS request,
    /// acknowledgement and stop.
    ///
    /// The cards the page's frames hold are let go of first, as
    /// `bobcat:frame-boot`'s `dispose` lets go of a frame's own, so each
    /// card's BTS has its `dispose` queued on the worker thread ahead of the
    /// page BTS's and runs its app cleanup before the page BTS acknowledges;
    /// then the page's own `__BobcatDispose`, whose acknowledgement
    /// [`Self::disposal_finished`] reads. The frames' acknowledgements are not
    /// waited for.
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

    /// Collects if enough held subtrees were removed since the last
    /// collection. Runs after every evaluation that succeeded; a failed one
    /// keeps its count for the next.
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

/// What every realm over this view's document installs its members over:
/// the page realm, and each of its `ShadowRealm` children, which are the
/// realms a `frame` renders.
///
/// One set of members for both (user ruling), and the same code: the same
/// slot, the same outbox and thread, the same listener table, the same
/// `createPage` and the same `flushElementTree`. The one record of which
/// page a realm renders is the nested page a child's `framePage` answered,
/// which a dispatch calls it at ([`DocumentSlot::roots`]): a child's
/// `bobcat:element` answers `__CreatePage` with the nested page
/// `bobcat:frame-boot` handed it (`__BobcatUsePageRoot`), and its card's
/// sheet goes to the frame's shadow root because `adoptStyleSheet` was told
/// so. What a child differs in is the registrant its event registrations are
/// filed under ([`ChildKey::listener_key`]), its `retired` flag, which its
/// `closeRealm` sets, what it does not get —
/// the startup strings answer `undefined`, and the native module members
/// are the page realm's, installed beside these by
/// [`MainThreadRuntime::new`] — and its workers: a child gets the same three
/// `Worker` members, over a worker owner of its own, which the child keeps
/// as its host state, so the workers it creates talk to it and end with it.
///
/// Held by every realm's `ShadowRealm` hook, so it holds no engine: the
/// slot, the outbox, a weak thread handle and a weak handle on the view's
/// workers only. The children's members hold clones of the slot, and the
/// children are freed with the realm core, before [`MainThreadRuntime`]'s
/// own `slot` field drops.
struct DocumentHost {
    slot: Rc<RefCell<DocumentSlot>>,
    outbox: ViewOutbox,
    thread: crate::jobs::JsThreadHandle,
    /// What a child's worker owner is made from. Weak, as the thread handle
    /// is: the page realm's owner holds it, and goes with the page realm.
    workers: std::rc::Weak<super::workers::ViewWorkers>,
}

impl DocumentHost {
    /// Installs the document members into one realm, with `startup` as its
    /// one-shot strings, `key` as the registrant its event registrations are
    /// filed under — [`PAGE_REALM`], or a child's
    /// [`ChildKey::listener_key`] — and `retired` as the flag its
    /// `closeRealm` sets: `None` for the page realm, which cannot close
    /// itself.
    fn install_members(
        &self,
        engine: &mut ScriptEngine,
        startup: [(&'static str, Option<String>); 4],
        key: ListenerKey,
        retired: Option<Rc<Cell<bool>>>,
    ) -> Result<(), MainThreadError> {
        install_host_module(
            engine,
            &self.slot,
            self.thread.clone(),
            key,
            retired.clone(),
        )?;
        install_event_members(engine, &self.slot, key, retired)?;
        style_sheets::install_styles(engine, &self.slot, &self.outbox, self.thread.clone())?;
        install_startup_strings(engine, startup)
    }
}

impl ChildHostModules for DocumentHost {
    /// A child's members: the page realm's, with no startup strings, which
    /// are the page's, its event registrations filed under `key`'s
    /// [`ChildKey::listener_key`], and a `closeRealm` that retires the child;
    /// then the `Worker` members over a worker owner of the child's, recorded
    /// under `key` so the workers it creates are delivered to it. What the
    /// child keeps is that owner and the release of its registrations
    /// ([`ChildDocumentState`]).
    fn install(
        &self,
        engine: &mut ScriptEngine,
        key: ChildKey,
        retired: Rc<Cell<bool>>,
    ) -> Result<Box<dyn ChildHostState>, ScriptError> {
        self.install_members(
            engine,
            STARTUP_STRINGS.map(|name| (name, None)),
            key.listener_key(),
            Some(retired),
        )
        .map_err(MainThreadError::into_script_error)?;
        // Gone only with the page realm's owner, while the page realm is
        // released, when no JavaScript runs to construct a child.
        let view = self.workers.upgrade().ok_or_else(|| {
            crate::threads::platform_script_error(
                "the view's workers have been released".to_owned(),
            )
        })?;
        let workers = view.owner(Some(key));
        workers
            .install(engine)
            .map_err(|error| context_of("installing Worker", error))?;
        Ok(Box::new(ChildDocumentState {
            workers: Box::new(workers),
            slot: Rc::clone(&self.slot),
            key: key.listener_key(),
        }))
    }
}

fn install(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, Box::new(callback))
        .map_err(|error| MainThreadError::from_engine("installing the host module", error))
}

/// Installs native host-module exports that parse their arguments, borrow the
/// slot, and run against the realm's document. Each `$parser` is one of the
/// argument helpers below, applied at the argument's position; `NAME` is the
/// diagnostic prefix every helper and validator stitches into its error.
///
/// The document is taken unconditionally: see [`DocumentSlot::document_mut`].
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
    // `flushElementTree`'s, for the waits it makes on the listed author sheets
    // and before a painter has bound.
    thread: crate::jobs::JsThreadHandle,
    // The registrant the realm's event registrations are filed under, which
    // its `framePage` and `closeRealm` name.
    key: ListenerKey,
    // A child's own release flag, which its `closeRealm` sets; `None` in the
    // page realm.
    retired: Option<Rc<Cell<bool>>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        fn createPage() |document| {
            Ok(node_id_value(document.document_element().id()))
        }
        fn createElement(tag: string_argument) |document| {
            Ok(node_id_value(document.create_element(tag, ())))
        }
        fn parentNode(node: node_id_argument) |document| {
            let parent = document.get(node).and_then(dom::Node::parent_id);
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

    // The two removals are written out rather than generated, because each
    // also counts toward the next collection: a detached subtree is freed
    // only once the handles naming it are finalized.
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
        if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
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
    // The realm's handle for `node` has been collected, and a handle is the
    // one thing that holds an element: the node is freed now. Only the node —
    // its element children are unlinked and go on as detached roots, each
    // held by the handle that names it. Host-owned text children are freed
    // with the node because no realm handle names them. Every
    // listener the realm had on it is gone too, since those lived on the
    // handle, and the listener table drops the node's rows in the
    // document's free path: the realm tells the host nothing about them.
    let closed = retired.clone();
    install(engine, "dropElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.dropElement";
        // A closed child's handles may still be finalized before the
        // epilogue frees the realm, naming nodes `resetFrame` has already
        // freed: nothing of the realm's is the document's to settle any more.
        if closed.as_ref().is_some_and(|closed| closed.get()) {
            return Ok(HostValue::Undefined);
        }
        let node = node_id_argument(NAME, arguments, 0)?;
        let mut tree = borrow_slot(NAME, &tree)?;
        let document = tree.document_mut();
        validate_removable(document, NAME, node)?;
        // A connected element's handle is held by its parent's, up to the
        // permanent page handle, so a connected element can never be the
        // subject of a drop; if one is, the graph and the tree disagree and
        // the realm must hear about it before the element is gone.
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
    install(engine, "flushElementTree", 0, move |_arguments| {
        borrow_slot("bobcat-internal:host.flushElementTree", &tree)?.flush(&thread)?;
        Ok(HostValue::Undefined)
    })?;

    install_nested_page_members(engine, handle, key, retired)
}

/// Installs the three members a nested card's realms speak to, all generic:
/// `framePage` reads a `frame`'s nested page, `resetFrame` empties it, and
/// `closeRealm` retires the realm that calls it. Which realm renders which
/// frame, and when one is let go of, is `bobcat:frame`'s to decide. The one
/// record kept is the page a child asked for, which `bobcat:frame-boot`
/// makes the child's page root: the step at that page is where a dispatch
/// calls the child ([`DocumentSlot::roots`]), until its `closeRealm` lets go
/// of it and of every registration the child filed.
fn install_nested_page_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
) -> Result<(), MainThreadError> {
    // The nested page a `frame` component built with the element, which
    // lives as long as the frame. Refused past `MAX_FRAME_DEPTH` frames, so a
    // card that embeds itself stops there. A child's answer is its root from
    // then on; the page realm's root is the document element, whatever
    // frames it reads.
    let tree = Rc::clone(handle);
    install(engine, "framePage", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.framePage";
        let frame = node_id_argument(NAME, arguments, 0)?;
        let mut slot = borrow_slot(NAME, &tree)?;
        let document = slot.document_mut();
        validate_live_element(document, NAME, frame)?;
        let page = nested_page(document, frame)
            .ok_or_else(|| format!("{NAME} requires a frame element"))?;
        if frame_depth(document, frame) > MAX_FRAME_DEPTH {
            return Err(format!(
                "{NAME}: frames nest at most {MAX_FRAME_DEPTH} deep"
            ));
        }
        if key != PAGE_REALM {
            slot.roots.insert(key, page);
        }
        Ok(node_id_value(page))
    })?;

    tree_members! { engine, handle;
        // Frees every child of a frame's nested page with its whole subtree
        // — it is inside a shadow tree, so `dom` frees what is under it
        // outright — and every sheet mounted on the frame's shadow root, so
        // the card loaded next is styled by its own alone. The nested page
        // stays.
        fn resetFrame(frame: node_id_argument) |document| {
            validate_live_element(document, NAME, frame)?;
            let page = nested_page(document, frame)
                .ok_or_else(|| format!("{NAME} requires a frame element"))?;
            let shadow = document
                .shadow_root(frame)
                .expect("a nested page sits in its frame's shadow root");
            let children: SmallVec<[dom::NodeId; 8]> = document
                .get(page)
                .map(|page| page.child_ids().iter().copied().collect())
                .unwrap_or_default();
            for child in children {
                document.drop_element(child);
            }
            document.clear_shadow_stylesheets(shadow);
            Ok(HostValue::Undefined)
        }
    }

    // Retires the child, and lets go of its root and of every registration
    // it filed now rather than when the release step frees it: from here on
    // no dispatch calls it, and a name only it held closes on the painter.
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

/// Installs the one member that begins the document's life.
///
/// It is the realm's, not the host's: `bobcat:element`'s `Document`
/// constructor calls it, and it resolves from any module in the realm —
/// `bobcat-internal:host` is realm-wide, as it is for `dropElement` — so a
/// card can reach it and be refused. There is no member at the other end:
/// nothing in the realm releases the document, which goes when the realm
/// does.
fn install_document_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    const NAME: &str = "bobcat-internal:host.createDocument";
    let tree = Rc::clone(handle);
    install(engine, "createDocument", 5, move |arguments| {
        // The five switches, in `PageConfig`'s order, as the boot module
        // wrote them. Read rather than passed through because Rust itself
        // needs the fields: the UA cascade and the document's own switches are
        // built out of them. A missing or non-boolean one fails the
        // construction, which fails the boot.
        let config = PageConfig {
            default_display_linear: boolean_argument(NAME, arguments, 0)?,
            default_overflow_visible: boolean_argument(NAME, arguments, 1)?,
            enable_css_selector: boolean_argument(NAME, arguments, 2)?,
            enable_js_data_processor: boolean_argument(NAME, arguments, 3)?,
            enable_css_inheritance: boolean_argument(NAME, arguments, 4)?,
        };
        // The page id is not this member's answer: `createPage` is still what
        // hands the realm the permanent root, and it now has a document to
        // read it from.
        borrow_slot(NAME, &tree)?.create_document(config)?;
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs `initData`, `globalProps`, `initialProcessor` and
/// `nativeModuleTable`, handing the realm the original strings. Missing
/// initial data or props become `undefined`.
///
/// Each hands its string over once and keeps nothing. `bobcat:runtime` parses
/// the initial data and props, uses the processor name as a plain string,
/// and posts the module table unread to the BTS in the `initialize` message.
/// All four answer before a document exists.
///
/// The strings are *moved* in rather than copied: each is handed over once and
/// never read again, so the move says what the lifetime is.
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

/// The names of the four one-shot strings, in the order they are installed.
/// A frame realm has all four, each answering `undefined`.
const STARTUP_STRINGS: [&str; 4] = [
    "initData",
    "globalProps",
    "initialProcessor",
    "nativeModuleTable",
];

/// The four strings this realm answers once, in the order they are
/// installed, taken out of the startup that is being spent.
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

/// Installs the two members a realm's event registrations speak to, for the
/// realm whose registrations are filed under `key`.
///
/// `listenerOpened(node, name, kinds)` says the realm now listens for `name`
/// on `node` in the passes `kinds` names — the bits of
/// [`ListenerKinds`]: 1 a bubble-pass registration (`__AddEventListener`'s
/// bubble list, `bindEvent`, `catchEvent`), 2 a capture-pass one (its
/// capture list, `capture-bind`, `capture-catch`), 4 a `global-bindEvent` —
/// replacing what it said there before, so a realm whose kinds change
/// reports the new set and nothing else. `listenerClosed(node, name)` says
/// it no longer listens there at all. The callbacks stay in the realm; what
/// crosses is one call per (element, name) edge, which
/// [`DocumentSlot::open_listener`] files in the document's listener table:
/// the table is what a dispatch reads to decide which realms to call and
/// where, what a component asks whether anything listens on it, what the
/// painter's name set follows, and what registers an element for exposure.
///
/// A node that is no longer live registers nothing, which is the table's
/// rule, and a realm that has closed (`retired`) is not heard at all: its
/// rows are gone, and nothing it says after that may file new ones. A freed
/// node's rows leave with it, in the document's free path, so a collected
/// handle tells the host nothing.
fn install_event_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
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

    Ok(())
}

/// Whether `name` is one of the two an element hears its exposure by, so a
/// change of its registrations is a change of whether the element is
/// registered for exposure detection.
fn is_exposure_event(name: &str) -> bool {
    name == APPEAR_EVENT || name == DISAPPEAR_EVENT
}

/// Whether some realm registered for `uiappear` or `uidisappear` on `node`,
/// in any form: what registers an element for exposure whether or not it
/// carries an `exposure-id`. Every form counts, `__AddEventListener`'s
/// included — native's rule: `FiberAddEventListener` syncs the name into the
/// element's event set for every form it files
/// (`core/runtime/lepus/bindings/renderer_functions.cc:1077`), and native
/// exposure fires `uiappear` at an element whose event set has it
/// (`UIExposure.java:543-547`). web-core marks an element from `__AddEvent`
/// alone (`createElementAPI.ts:160-170`).
fn hears_exposure(document: &LynxDocument, node: dom::NodeId) -> bool {
    document.has_listener(node, APPEAR_EVENT) || document.has_listener(node, DISAPPEAR_EVENT)
}

/// What a child's host modules keep for it: its `Worker` owner, and the
/// release of its event registrations, which [`Drop`] runs when the child is
/// freed — so a child freed without closing itself (a `ShadowRealm` nothing
/// referred to any more) leaves no row in the listener table and no root,
/// as one that called `closeRealm` already left none.
struct ChildDocumentState {
    workers: Box<dyn ChildHostState>,
    slot: Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
}

impl ChildHostState for ChildDocumentState {
    fn is_busy(&self) -> bool {
        self.workers.is_busy()
    }
}

impl Drop for ChildDocumentState {
    fn drop(&mut self) {
        // A child is freed by the owner's release step or with the realm
        // core, never inside a member, so nothing holds the slot here.
        if let Ok(mut slot) = self.slot.try_borrow_mut() {
            slot.forget_realm(self.key);
        }
    }
}

/// The UI-method status codes `callElementMethod` answers with, from the
/// table every `invoke` path reports (native's `LynxGetUIResult`,
/// `lynx/core/renderer/dom/lynx_get_ui_result.h:53-61`; web-core's
/// `ErrorCode`, `web-core/ts/constants.ts:83-91`). 7 and 8 are native's alone:
/// web-core reports every method that throws as 4.
const UI_METHOD_SUCCESS: f64 = 0.0;
const UI_METHOD_NOT_FOUND: f64 = 3.0;
const UI_METHOD_PARAM_INVALID: f64 = 4.0;
const UI_METHOD_INVALID_STATE: f64 = 7.0;
const UI_METHOD_OPERATION_ERROR: f64 = 8.0;

/// What `callElementMethod` answers for a method's outcome, a kind's or the
/// base set's: the status code, or for `Data` a record whose first field is
/// the code `0` followed by the method's fields unchanged, which
/// `element-papi.ts` reads by method name (`boundingClientRect`, a
/// `scroll-view`'s and a `list`'s data methods, and a `text`'s
/// `getTextBoundingRect`), so the realm sees one of two shapes: a number, or
/// a record.
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

/// Installs the two members that read geometry and style back out of the
/// document, one of which also runs the UI methods.
///
/// Neither runs a pipeline step. Both read what the last completed pass
/// left behind, `boundingClientRect` and a `text`'s `getTextBoundingRect`
/// included, and the realm decides when the next one runs by calling
/// `__FlushElementTree` — measuring must not be able to move layout out from
/// under the job that measures, and a card that wants current numbers says
/// so. The UI methods that write record what the next commit carries:
/// `selectTab`, a `scroll-view`'s `scrollTo` and `scrollBy`, a `list`'s
/// `scrollToPosition` and `scrollBy`, an `x-swiper`'s `scrollTo`, an
/// `x-refresh-view`'s `finishRefresh`, `finishLoadMore` and
/// `autoStartRefresh`, a scroll coordinator's `setFoldExpanded`, and every
/// element's `scrollIntoView`, `scrollTo` and `scrollBy` a scroll request, a
/// dialog's four an attribute, a state bit and top-layer membership, and
/// `close`/`requestClose` the events they queue for an entry of their own. Both still go through
/// [`validate_live_element`], so a freed element is a script error rather than a zero rect or an
/// empty style.
fn install_readback_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    // Written out rather than generated, because the answer is either a
    // number or a string.
    let tree = Rc::clone(handle);
    install(engine, "callElementMethod", 3, move |arguments| {
        const NAME: &str = "bobcat-internal:host.callElementMethod";
        let node = node_id_argument(NAME, arguments, 0)?;
        let method = string_argument(NAME, arguments, 1)?;
        let params = string_argument(NAME, arguments, 2)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_live_element(document, NAME, node)?;
        // Every name is the element kind's own method first
        // (`dom::CustomElement::invoke`, run by the kind's component in
        // `tree`). The kinds' methods built are a pager's `selectTab`
        // (`tree::viewpager`), a `scroll-view`'s `scrollTo`, `scrollBy`
        // and `getScrollInfo` (`tree::scroll_container`), a `list`'s
        // `scrollToPosition`, `scrollBy`, `getScrollContainerInfo` and
        // `getVisibleCells` (`tree::list`), an
        // `x-swiper`'s `scrollTo` (`tree::swiper`), an `x-refresh-view`'s
        // `finishRefresh`, `finishLoadMore` and `autoStartRefresh`
        // (`tree::refresh_view`), a scroll coordinator's
        // `setFoldExpanded` (`tree::scroll_coordinator`), a `text`'s
        // `getTextBoundingRect` (`tree::text`), and a dialog's
        // `show`, `showModal`, `close` and `requestClose`
        // (`tree::dialog`). On `NotFound` the shared base set every
        // element has answers (`tree::base_methods`): the documented UI
        // methods `boundingClientRect`, `scrollIntoView`,
        // `scrollTo`/`scrollBy` and `focus`/`blur` (user ruling
        // 2026-10-09), of which all but `focus`/`blur` are built; a name
        // neither answers is code 3. No name is matched here. A kind's
        // `scrollTo`/`scrollBy` shadows the base set's, and no kind
        // answers `boundingClientRect`, so it is the same method on every
        // element, as web-core answers it ahead of the element
        // (`createInvokeUIMethod.ts:16-28`). The kind's method runs
        // inside the `[CEReactions]` scope `invoke_element_method` opens
        // and the base method outside it, which needs none: the base set
        // records scroll requests and reads geometry, and queues no
        // custom element reaction.
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
        fn getComputedStyleMap(
            node: node_id_argument,
            properties: string_argument,
            resolved: flag_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            // An empty payload is "every property", so it must not become the
            // one-element filter `"".split(',')` would produce.
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

/// Installs the DOM-attribute and inline-style portion of the host module.
///
/// `id`, `class`, and `style` deliberately travel through the same narrow
/// string boundary as every other attribute. [`LynxDocument`] owns their
/// specialized DOM/style invalidation paths. `setInlineStyles` is the one
/// wider primitive: a whole record in one crossing, because a record is a
/// whole-block replacement and building it from empty is what the setter
/// means. Nothing in the realm — the Element PAPI included — receives a
/// document handle.
///
/// An attribute a tag reflects into CSS — `text-maxline`, `span-count`,
/// `src` — is reflected by that tag's own component, in the
/// `attribute_changed_callback` [`LynxDocument`] raises and drains inside the
/// write below. This layer only performs the DOM mutation: it neither knows
/// which names mean something nor which tag they mean it on — with the one
/// exception [`install_attribute_writes`] makes for the exposure attributes.
fn install_attribute_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    install_attribute_writes(engine, handle)?;

    tree_members! { engine, handle;
        // Deliberately name-based: this PAPI receives record keys, custom
        // properties have no numeric id, and Stylo's internal PropertyId is
        // not a stable script ABI. A future numeric-key `__AddInlineStyle`
        // can translate its bundle id in JavaScript/the decoder-owned layer
        // before reaching this one primitive.
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
            // SelectorQuery is Lynx's inclusive query scope, so it asks for
            // the root to be considered; the Element PAPI's `__QuerySelector`
            // is `Element.querySelector`'s, which is root-exclusive, and asks
            // for it to be skipped. Matching the root itself still uses the
            // same standard selector engine as the cascade.
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
            let value = element.attribute(name).map(str::to_owned);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            let tag = validate_live_element(document, NAME, node)?
                .tag_name()
                .ok_or_else(|| {
                    "bobcat-internal:host.tagName requires a live element tag".to_owned()
                })?;
            Ok(HostValue::String(tag.to_owned()))
        }
        fn attributeNames(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut record = String::new();
            for (name, _) in element.attributes() {
                write_record_field(&mut record, name);
            }
            Ok(HostValue::String(record))
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

/// Installs `setAttribute` and `removeAttribute`, written out rather than
/// generated because each tells the slot when the name is an exposure
/// attribute.
///
/// That is the one exception to "this layer knows no attribute names". The
/// exposure attributes (`exposure-id`, `exposure-scene`, `exposure-area`, the
/// screen and ui margins) belong to every tag, so no tag's component can own
/// them, and what they drive — the
/// document's exposure registrations — sits beside the document in the slot,
/// out of a component's reach. The slot is told after the mutation, so the
/// registration reads the attributes as they now are.
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
        }
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs the member the realm's exposure switch speaks to, answered by
/// the document's [`Exposure`] beside the document in the slot.
///
/// `switchExposure(on, sendEvent)` is `lynx.stopExposure` /
/// `lynx.resumeExposure` as native runs them. Both flags are `0` or `1`, as
/// every flag a tree member takes, and it answers nothing: what it changes
/// is delivered by the next intersection delivery entry. Whether an element
/// is registered for its `uiappear`/`uidisappear` is not a member's: the
/// listener table answers it ([`DocumentSlot::open_listener`]).
fn install_exposure_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "switchExposure", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.switchExposure";
        let on = flag_argument(NAME, arguments, 0)?;
        let send_event = flag_argument(NAME, arguments, 1)?;
        borrow_slot(NAME, &tree)?.switch_exposure(on, send_event);
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Splits a `__SetInlineStyles` record payload into its declarations.
///
/// The payload is a flat sequence of `<units>:<text>` fields, two per
/// declaration — the hyphenated property name, then the value. `<units>` is
/// the text's length in UTF-16 code units, which is exactly what JavaScript's
/// `String.prototype.length` reports, so the writing side needs no scan and
/// no escaping.
///
/// Length-prefixing rather than delimiting is the point: a declaration value
/// is arbitrary author text, and any separator this could have used — a
/// semicolon, a NUL, a private-use code point — is a character some value may
/// legitimately contain. A length says where the next field starts without
/// asking what is inside this one.
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

/// Reads one `<units>:<text>` field, returning it and what follows:
/// [`write_record_field`]'s inverse.
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
    // Short of the count means the payload was truncated; past it means the
    // count landed inside a surrogate pair. Neither can come from the writer.
    if counted != units {
        return Err(malformed());
    }
    Ok((&body[..end], &body[end..]))
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
    node: dom::NodeId,
) -> Result<&'a dom::Node<()>, String> {
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
    node: dom::NodeId,
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
    parent: dom::NodeId,
    child: dom::NodeId,
    reference: Option<dom::NodeId>,
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
    a: dom::NodeId,
    b: dom::NodeId,
) -> Result<(), String> {
    if a == b {
        return Err(format!("{function} requires distinct elements"));
    }
    validate_removable(document, function, a)?;
    validate_removable(document, function, b)?;
    if document.get(a).and_then(dom::Node::parent_id).is_none()
        || document.get(b).and_then(dom::Node::parent_id).is_none()
    {
        return Err(format!("{function} requires attached elements"));
    }
    if document.is_ancestor(a, b) || document.is_ancestor(b, a) {
        return Err(format!("{function} cannot swap an ancestor and descendant"));
    }
    Ok(())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

/// The largest integer an `f64` represents exactly. A packed `NodeId` is built
/// to stay under it, so a handle survives the script boundary unchanged.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// A `NodeId` crossing into script *is* the element's Lynx `unique_id`: the DOM
/// issues it, and `__GetElementUniqueID` hands back the same number the
/// creating PAPI returned.
///
/// The handle carries both the arena key and the generation that key was at, so
/// it crosses packed into one integer. The generation is what makes a stale
/// handle safe: script holds these for as long as it likes, and a number it
/// stashed can outlive the element it named (a released element is freed once
/// it is detached), so such an id must resolve to nothing rather than to
/// whatever took its place.
#[allow(
    clippy::cast_precision_loss,
    reason = "a packed handle is built to stay inside f64's exact-integer range"
)]
fn packed_node_id(node: dom::NodeId) -> f64 {
    node.to_bits() as f64
}

/// The same handle as a value a host callback returns, rather than as an
/// argument the runtime lends into a call.
fn node_id_value(node: dom::NodeId) -> HostValue {
    HostValue::Number(packed_node_id(node))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds and integer checks above make the value a representable handle"
)]
fn node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<dom::NodeId, String> {
    let HostValue::Number(value) = *argument(arguments, index) else {
        return Err(format!("{function} expects a number for argument {index}"));
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= MAX_EXACT_INTEGER {
        return Err(format!(
            "{function} expects a non-negative integer node id for argument {index}"
        ));
    }
    dom::NodeId::from_bits(value as u64)
        .ok_or_else(|| format!("{function} got a number that is no element id: {value}"))
}

fn optional_node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<Option<dom::NodeId>, String> {
    match *argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(None),
        _ => node_id_argument(function, arguments, index).map(Some),
    }
}

/// A flag the realm spells as `0` or `1`, which is every boolean a tree
/// member takes.
fn flag_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Number(0.0) => Ok(false),
        HostValue::Number(1.0) => Ok(true),
        _ => Err(format!("{function} expects 0 or 1 for argument {index}")),
    }
}

/// The passes one registration runs in, as `listenerOpened` takes them: the
/// bits of [`ListenerKinds`], at least one. An empty set is not an opening —
/// `listenerClosed` is the removal — so 0 is refused with every non-integer
/// and every bit outside the three.
fn kinds_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<ListenerKinds, String> {
    match *argument(arguments, index) {
        HostValue::Number(value) if value.fract() == 0.0 && (1.0..=7.0).contains(&value) =>
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

/// A JavaScript boolean, which is what the boot module hands `createDocument`
/// its five switches as.
fn boolean_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Boolean(value) => Ok(value),
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
