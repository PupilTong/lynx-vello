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
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use terminus::link::event::{EventDetail, ListenerKey, ListenerKinds, RegistrationChange};
use terminus::link::{MethodCall, MethodError, MethodOutcome, StylePool};
use tokio::sync::watch;

use self::pipelines::{FlushRequest, RealmTiming, TIMING_FLAG_ATTRIBUTE};
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
    Autoplays, ComponentDetail, ComponentEvent, ComponentEvents, Field, LynxDocument,
    MAX_FRAME_DEPTH, PageConfig, SELECTION_CHANGE, SelectionReports, frame_depth,
    invoke_base_method, is_svg, is_text, nested_page, new_document, report_selection_changes,
    reset_frame,
};
use crate::performance::OpenerId;
use crate::realm::child::{ChildHostModules, ChildHostState, ChildKey};
use crate::realm::policy::context_of;
use crate::realm::{RealmCore, RealmId, open_realm, string_argument};
use crate::script::ScriptError;
use crate::timers::run_due_timers;
use crate::view::{LynxViewError, ScreenMetrics, ScriptSource, StartupSource, Viewport};

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";

/// web-core's rule for which events a nested card hears from the whole
/// document: the names web-core listens for on `document` rather than on the
/// card's `lynx-view` shadow root (`DOCUMENT_LEVEL_EVENTS`,
/// `web-core/ts/client/mainthread/elementAPIs/WASMJSBinding.ts:30`, used at
/// `:266-282`), intersected with the names this engine dispatches. For these,
/// every realm with a `global-bindEvent` registration for the name is
/// invited whatever tree the registering node is in, so a frame's card hears
/// every key, whose target is the document element; every other global
/// registration is reached only by a path through its node's tree root
/// ([`terminus::link::Document::global_registrants_on_path`]). See [`Invited`].
const DOCUMENT_LEVEL_EVENTS: [&str; 2] = ["keydown", "keyup"];

/// The internal events a `frame`'s connection and disconnection are: the
/// names the realm's Lynx layer files its own listener under on every
/// `frame` it creates, so the listener table invites the realm that holds
/// the frame's handle, whatever its depth. Dispatched at the frame,
/// non-bubbling and not composed, with no payload; see
/// [`MainThreadRuntime::dispatch_component_events`]. The `bobcat:` prefix
/// keeps them apart from every name an app can file.
const FRAME_CONNECTED_EVENT: &str = "bobcat:frameconnected";
const FRAME_DISCONNECTED_EVENT: &str = "bobcat:framedisconnected";
/// What an element hears when its exposure moves. web-core's names
/// (`ExposureServices.ts:209-220`), dispatched non-bubbling and not composed
/// as web-core dispatches them.
const APPEAR_EVENT: &str = "uiappear";
const DISAPPEAR_EVENT: &str = "uidisappear";

/// The internal event an element's exposure record is: the name the realm's
/// Lynx layer files its record listener under, on every element it registers
/// for exposure, and so the listener-table row that registers the element
/// here ([`Exposure::set_registered`]). The `bobcat:` prefix keeps it apart
/// from every name an app can file.
///
/// Dispatched at the element, non-bubbling and not composed, with no W3C
/// payload and three extras the listener builds the record from:
/// `exposureID`, `exposureScene` and `appeared` (true for an `exposure`
/// record, false for a `disexposure`); see
/// [`MainThreadRuntime::notify_intersection_observers`].
const EXPOSURE_RECORD_EVENT: &str = "bobcat:exposure";

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
/// [`DETAIL_CLONED`] and [`DETAIL_DISPATCHED`] are no [`EventDetail`]'s:
/// they are a synthetic event's, the script `detail` as the structured clone
/// the dispatching realm wrote for every other realm, and the mark that
/// tells the dispatching realm to walk its own event object
/// ([`CallDetail`]).
const DETAIL_POSITION: f64 = 0.0;
const DETAIL_EMPTY: f64 = 2.0;
const DETAIL_EXPOSURE: f64 = 3.0;
const DETAIL_KEY: f64 = 4.0;
const DETAIL_INPUT: f64 = 5.0;
const DETAIL_COMPOSITION: f64 = 6.0;
const DETAIL_FOCUS: f64 = 7.0;
const DETAIL_CLONED: f64 = 8.0;
const DETAIL_DISPATCHED: f64 = 9.0;

/// The bits a key event's modifiers cross as, in [`DETAIL_KEY`]'s last
/// number. Mirrored by `element-papi.ts`.
const MODIFIER_ALT: u8 = 1;
const MODIFIER_CTRL: u8 = 2;
const MODIFIER_META: u8 = 4;
const MODIFIER_SHIFT: u8 = 8;

/// One dispatch's extras: names, each with its value. Empty for nearly
/// every event.
///
/// The extras channel carries what a Lynx handler reads beside a W3C
/// event's own payload, when it is cheap to compute here (a `load`'s size,
/// a `scroll`'s metrics, an `x-input`'s `value`). The kernel's events stay
/// W3C-shaped ([`EventDetail`]); the realm's Lynx layer turns a dispatch's
/// extras into the Lynx `detail`, reading one primitive per key: only a
/// [`HostArgument::Number`], a [`HostArgument::Boolean`] or a
/// [`HostArgument::String`] crosses here, never a structured clone,
/// `undefined` or `null`. A string borrows a component's for the length of
/// the dispatch: a [`ComponentDetail`]'s url, content or content id, or a
/// text field's value.
type Extras<'a> = &'a [(&'static str, HostArgument<'a>)];

/// Appends a [`ComponentDetail`] as extras, one per key of the Lynx
/// `detail`, under the keys and in the member order web-core's details
/// carry: `<x-markdown>`'s `id`; `url`, `content` and `contentId`; `url`
/// and `contentId` — an `id` or `contentId` the component had no value for
/// pushes nothing, so the realm's `detail` has no such key — and a text
/// field's (`XInputEvents.ts`) `value`; `value`, `textLength`, `cursor`,
/// `isComposing`, `selectionStart` and `selectionEnd`, where `textLength`
/// is the value's UTF-16 length and the deprecated `cursor` the selection's
/// start; `selectionStart` and `selectionEnd`; and a Lynx text element's
/// (`XTextSelectionEvents.ts`) `start`, `end` and `direction`, `-1`, `-1`
/// and `"forward"` for the empty record. Seven extras at most, which the
/// inline capacity holds. A [`ComponentDetail::Fields`] pushes its pairs as
/// they are, in its order.
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

/// A `text`'s lines as the `lines` extra of its `layout` event carries them:
/// JSON text of one flat array, `start, end, ellipsisCount` per line, in
/// line order — `[0,6,0,6,11,0]` for two lines. Rust owns these numbers and
/// the realm's Lynx layer builds the objects (`AGENTS.md`, "JavaScript data
/// ownership"); a string is how a list crosses the extras channel, which
/// carries one primitive per key.
fn text_lines_json(lines: &[terminus::link::TextLineReport]) -> String {
    let mut text = String::with_capacity(2 + lines.len() * 12);
    text.push('[');
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{},{},{}", line.start, line.end, line.ellipsis_count);
    }
    text.push(']');
    text
}

/// Appends a `text`'s `layout` as extras: `lineCount`, `lines` — the JSON
/// text [`text_lines_json`] wrote, which the realm's Lynx layer parses into
/// `{start, end, ellipsisCount}` objects — and the content box's `width` and
/// `height`, from which the realm builds `{lineCount, lines, size}`.
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

/// Inline arguments for one dispatch, in call order: the name, `bubbles`,
/// the timestamp and the step count; two node ids per step; the extras
/// count; an input event's discriminator and eight detail numbers. That is
/// 31 for a position event over eight steps; extras, touch points and
/// longer paths spill. Encoded once per dispatch ([`encode_call`]) and lent
/// to every realm it calls.
type Arguments<'a> = SmallVec<[HostArgument<'a>; 32]>;

/// The registrants a dispatch of `name` over `steps` invites, as the
/// document's listener table holds them when this is read: each with a path
/// registration for the name that the path's passes reach
/// ([`terminus::link::Document::listeners_on_path`]), or with a `global-bindEvent`
/// registration for it that the path reaches — one whose node's tree root
/// (the document node, or the shadow root it sits in) is a node of the path
/// ([`terminus::link::Document::global_registrants_on_path`]) — or, for a
/// [`DOCUMENT_LEVEL_EVENTS`] name, with a `global-bindEvent` registration
/// for it anywhere ([`terminus::link::Document::global_registrants`]), which includes
/// every one the path reaches.
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

/// The registrants a dispatch of `name` over `steps` calls, in the order
/// they are called, which is the order of `registrants`: each one
/// [`Invited`]. Every one is called at the path's top step — a path always
/// has one capture step, its target's own — and handed the same
/// [`Arguments`]. See [`MainThreadRuntime::dispatch`].
///
/// Decided while the document is borrowed and before the walk takes a step,
/// so they are read against the tree the path was computed over; the
/// [`Invited`] they were read from is answered beside them.
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

/// One realm call's arguments for a dispatch, which every realm it calls
/// is lent: the name, `bubbles` and the timestamp, the path
/// ([`encode_path`]), the extras ([`encode_extras`]) and the detail, last
/// because a touch event's points make it the one part of variable length
/// besides the extras before it.
///
/// Nothing here is read from the tree but the path, and that only at the
/// path's top step, where every realm is called, so one encoding is the
/// list every call of the dispatch would have built for itself. A synthetic
/// dispatch encodes a second one for the realm that dispatched it, which
/// differs in the detail alone ([`CallDetail::Dispatched`]).
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

/// What one realm call of a dispatch carries after the path.
#[derive(Clone, Copy)]
enum CallDetail<'a> {
    /// A routed event's extras ([`encode_extras`]) and W3C payload
    /// ([`encode_detail`]).
    Event {
        detail: &'a EventDetail,
        extras: Extras<'a>,
    },
    /// A synthetic event's script `detail`, for every realm but the one that
    /// dispatched it: no extras, then [`DETAIL_CLONED`] and the value as the
    /// dispatching realm lent it to its call — a structured clone for an
    /// object, which each realm reads back as its own copy — or
    /// [`DETAIL_EMPTY`] for an `undefined` or `null` one.
    Cloned(Option<HostArgument<'a>>),
    /// The same synthetic event, for the realm that dispatched it: no
    /// extras, then [`DETAIL_DISPATCHED`] alone. That realm walks the event
    /// object its `__DispatchEvent` was given, which it kept for this call,
    /// so it is handed no copy of what that object already holds.
    Dispatched,
}

/// The event one dispatch delivers: its target, its name, its two flags as
/// [`terminus::link::Document::event_steps`] takes them, and its timestamp.
struct Dispatch<'a> {
    target: terminus::link::NodeId,
    name: &'a str,
    bubbles: bool,
    composed: bool,
    timestamp: f64,
}

/// Where a dispatch came from, which is all the detail its parts are
/// handed depends on.
enum DispatchDetail<'a> {
    /// The kernel's ([`MainThreadRuntime::dispatch`]): the components' walk
    /// hears `detail`, and every realm is handed it with `extras`
    /// ([`CallDetail::Event`]).
    Routed {
        detail: &'a EventDetail,
        extras: Extras<'a>,
    },
    /// A realm's `__DispatchEvent` ([`install_synthetic_event_members`]),
    /// the realm filed under `own`: the components' walk hears
    /// [`EventDetail::None`], since the script `detail` is a realm value;
    /// `own` is handed [`CallDetail::Dispatched`] and every other realm
    /// [`CallDetail::Cloned`] with `detail`.
    Synthetic {
        own: ListenerKey,
        detail: Option<HostArgument<'a>>,
    },
}

/// The one dispatch body, which the kernel's dispatches
/// ([`MainThreadRuntime::dispatch_checkpointed`]) and a realm's synthetic
/// ones (the `dispatchEvent` member, [`install_synthetic_event_members`])
/// both run: what fires, in what order, is
/// [`MainThreadRuntime::dispatch`]'s, written here once.
///
/// A target freed since the event formed, or a name nothing hears — no
/// registration and no observing definition — answers `None` before any
/// path is built. Otherwise the path is computed once, the realms to call
/// are decided from the listener table against it ([`realm_calls`], over
/// `realms` in its order: the owner's realm, then the children in
/// construction order), and the walk ([`terminus::link::Document::advance`])
/// runs the components' handlers one step at a time with the slot borrowed
/// for that step alone. At the first capture step, the path's top, every
/// invited realm is called through `call`, in that order, each still
/// invited when its turn comes — re-read after each call that ran — and
/// handed the one argument list ([`encode_call`]) encoded right before the
/// first call; the realm that dispatched a synthetic event is handed its own
/// ([`CallDetail::Dispatched`]). The answer is whether some call answered
/// that the realm published the export.
///
/// `call` is the one thing the two entries differ in: the kernel's
/// checkpoints each realm right after its call (or defers it), and collects
/// what a call threw; the member's runs no checkpoint at all, since it runs
/// inside a JavaScript call, and reports what a call threw itself. Nothing
/// here holds the slot or anything of a realm's across `call`, so a
/// listener may mutate the tree, and may dispatch again, which enters this
/// body again from inside the call.
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
    let (steps, calls, invited) = {
        let slot = slot.borrow();
        let document = slot.document.as_ref().expect(DOCUMENT_EXISTS);
        // A registration is filed under its lowercased name, and the
        // components' walk looks a type up in its own spelling: those
        // are the two questions, each asked once.
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
    // Taken at the first capture step, the path's top, where every
    // invited realm is called.
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
        // The document is borrowed for one step's components alone: a
        // realm call below runs listeners that mutate the tree.
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
        // Encoded once, with the document borrowed for the encoding
        // alone, and lent to every call; the dispatching realm's beside it,
        // against the same tree.
        let (arguments, dispatching) = {
            let slot = slot.borrow();
            let document = slot.document.as_ref().expect(DOCUMENT_EXISTS);
            let encode = |detail| encode_call(document, &steps, name, bubbles, timestamp, detail);
            let dispatching = own
                .filter(|own| calls.contains(own))
                .map(|own| (own, encode(CallDetail::Dispatched)));
            (encode(call_detail), dispatching)
        };
        // Who is still invited, read from the table as the calls before
        // left it: a realm no longer invited has nothing left to run. Read
        // once for every realm up to the next call that runs, since
        // between two calls only a call's listeners change the table.
        //
        // The first call starts from the read that chose the calls: the
        // one thing that ran since is the first capture step's component
        // handler, at the path's top, and none of today's components
        // files or drops a registration or frees or moves a node there —
        // a refresh view's, a swiper's or a text field's handler at its
        // own shadow root reads the document and queues events. A
        // handler that does must start this from `None`.
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

/// The steps of a dispatch's path every realm is handed: the capture steps
/// read backwards — the path, target first, whatever `bubbles` cut from the
/// bubble pass — less every step whose node or target is no longer live and
/// every step at a shadow root, which no realm can hold a handle for.
/// Nothing is filtered by tree: a realm is handed nodes it holds no handle
/// for (another realm's, a UA component's shadow content) and skips them. A
/// `NodeId` names one node for the life of the document, so liveness is one
/// lookup per id. [`encode_path`] writes them into a realm call.
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

/// The one encoding of a dispatch's path into a realm call, appended as the
/// step count and then two packed ids per step ([`packed_node_id`]), the
/// step's node and the target that step sees, for the steps
/// [`realm_path_steps`] selects.
///
/// The count leads but is known only once the filter has run, so it is
/// written into the place kept for it, in the same one pass.
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

/// Appends the extras channel: the count, then each name and its value.
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

/// The W3C `PointerEvent.pointerType` of a device.
fn pointer_type(device: terminus::link::input::PointerKind) -> &'static str {
    match device {
        terminus::link::input::PointerKind::Touch => "touch",
        terminus::link::input::PointerKind::Pen => "pen",
        _ => "mouse",
    }
}

/// The script boundary's encoding of `terminus`'s [`EventDetail`]: the
/// discriminator, then the values that kind spends, in the order the realm
/// reads them. `terminus`'s detail is the one description of what an event
/// carries; this is only how it crosses, and the object is built in the
/// realm, because its shape is JavaScript's.
///
/// - [`DETAIL_POSITION`] for a pointer, wheel or touch event, eight values: the device position;
///   the wheel delta; W3C `button`, `buttons`, `pointerId` and `pointerType` — each `undefined` for
///   a shape that has none, which is how the realm tells the three apart — then four numbers per
///   touch point. The realm puts the W3C members on the event object.
/// - [`DETAIL_EXPOSURE`] for a `uiappear`/`uidisappear`: nothing. The realm builds web-core's
///   detail (`ExposureServices.ts:202-208`) from the target's own unique id and its `exposure-id`
///   and `exposure-scene` attributes, which are the live element's because
///   [`MainThreadRuntime::dispatch`] resolves the target before it calls the realm.
/// - [`DETAIL_KEY`] for `keydown`/`keyup`: the W3C `key` and `code` strings, borrowed from the
///   detail, `repeat` as a boolean and the modifiers as one bitmask. The realm puts them on the
///   event object itself, as `KeyboardEvent` and web-core do.
/// - [`DETAIL_INPUT`] for `beforeinput`/`input`: the W3C `inputType` string, `data` as a string or
///   `null`, and `isComposing` as a boolean — `InputEvent`'s members, which the realm puts on the
///   event object itself.
/// - [`DETAIL_COMPOSITION`] for `compositionstart`/`compositionupdate`/`compositionend`: the `data`
///   string, `CompositionEvent.data` on the event object.
/// - [`DETAIL_FOCUS`] for `focus`/`blur`/`focusin`/`focusout`: the `relatedTarget`'s packed node
///   id, or `null` when there is none. The realm resolves it to the element's descriptor, or to
///   `null` for an element script holds no handle of.
/// - [`DETAIL_EMPTY`] for everything else: nothing.
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
            // The typed `text` stays here: `KeyboardEvent` has no such member.
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
            // `None`, and the engine's own `ContentVisibility`, which never
            // reaches a realm.
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

/// Declarations one `__SetInlineStyles` record carries without touching the
/// heap. Compiled `ReactLynx` records are a handful of properties.
const INLINE_DECLARATIONS: usize = 16;

mod editing;
mod pipelines;
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

/// Where the realm calls of a dispatch checkpoint.
enum Checkpoint<'a> {
    /// Each call ends in its realm's microtask checkpoint, and the dispatch
    /// in [`MainThreadRuntime::finish_batch`]: every dispatch but the
    /// exposure records'.
    Each,
    /// No call checkpoints, and the dispatch does not end the batch: each
    /// realm called is added to the list, once, for the caller to
    /// checkpoint after its last dispatch
    /// ([`RealmCore::checkpoint_after_calls`]). So a run of dispatches is
    /// one checkpoint per realm, after all of them.
    Deferred(&'a mut SmallVec<[RealmId; 2]>),
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
    /// [`adopt_text_context`](terminus::link::Document::adopt_text_context). `None` is a
    /// view that named neither, which leaves the document's own lazy context
    /// alone.
    pub(crate) text_context: Option<terminus::link::TextContext>,
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
/// The deliveries one entry's [`MainThreadRuntime::settle_entry`] left
/// owing, each of which the page posts as an entry of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Deliveries(u8);

impl Deliveries {
    pub(crate) const NONE: Self = Self(0);
    /// Resize observers have entries queued.
    pub(crate) const RESIZE: Self = Self(1);
    /// Intersection observers have entries queued, or exposure changes wait.
    pub(crate) const INTERSECTION: Self = Self(1 << 1);
    /// A `content-visibility: auto` skipping change is owed.
    pub(crate) const CONTENT_VISIBILITY: Self = Self(1 << 2);
    /// Component events are owed, and nothing is left uncommitted.
    pub(crate) const COMPONENT_EVENTS: Self = Self(1 << 3);

    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every delivery `other` names is owed.
    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

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
    held: Option<Arc<terminus::link::CommittedFrame>>,
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
    /// The document's exposure registrations and the changes they owe
    /// script.
    ///
    /// Beside the document rather than inside it because exposure is Lynx's
    /// and `terminus` knows nothing of it: each registration is one of `terminus`'s
    /// intersection observers, and what those deliver is turned into
    /// exposure here and drained into the realm by the intersection delivery
    /// entry ([`MainThreadRuntime::notify_intersection_observers`]).
    exposure: Exposure,
    /// The document's `layoutchange` observer and what it delivered, beside
    /// the document for the reason `exposure` is: `layoutchange` is Lynx's,
    /// one of `terminus`'s resize observers carries it, and what that observer
    /// delivers is dispatched into the realm by the resize delivery entry
    /// ([`MainThreadRuntime::notify_resize_observers`]).
    layout_change: LayoutChangeObserver,
    /// Per text element, the last non-empty Lynx `selectionchange` record it
    /// was sent, which the `text` module compares the next one against
    /// ([`report_selection_changes`]). Beside the document for what
    /// `exposure` is beside it for: the event is Lynx's, and `terminus` keeps only
    /// the W3C selection it is derived from. An entry exists only for an
    /// element that was sent a non-empty record, and leaves when the element
    /// is sent the empty record or nothing listens on it any more, so the
    /// table holds the one or two text elements the document's one
    /// selection lies in.
    text_selection_reports: SelectionReports,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// Which [`editing::EditingRoute`]s the editing account routes in the
    /// painter's name set, one bit per route
    /// ([`editing::EditingRoute::bit`]). Beside the listener table rather
    /// than in it: the kernel is no realm, and a row of its would be an
    /// invitee [`realm_calls`] has no realm to call for.
    editing_routes: u8,
    /// The event names the document's components observe on an element that
    /// exists, as [`terminus::link::Document::take_observed_name_changes`] said when last
    /// drained ([`Self::sync_names`]). With the document's listener
    /// table ([`terminus::link::Document::has_listeners_for`]), what the painter routes
    /// against ([`Self::routes`]).
    observed: FxHashSet<&'static str>,
    /// The scroll containers that have scrolled since the painter last
    /// posted them at rest: an adopted offset moved them, or a script-facing
    /// instant scroll did. Kept whether or not anything listens, so a
    /// `scrollend` listener added mid-scroll still hears the end; a rest
    /// takes a container out, and so does its node being freed. Bounded by
    /// the containers scrolling at once.
    scrolling: SmallVec<[terminus::link::NodeId; 4]>,
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
    ///
    /// On the slot rather than on [`MainThreadRuntime`] because a realm reads
    /// it too, through `timelineMilliseconds`, and a member reaches the slot
    /// alone: `__CreateEvent` stamps the event it builds with it.
    timeline_milliseconds: f64,
    /// The `__lynx_timing_flag` attribute values written since the last
    /// timed flush, which the next flush binds to its commit
    /// ([`pipelines`]). Empty on a page that sets none.
    pending_flags: Vec<Arc<str>>,
    /// The native-triggered pipelines opened since the last timed flush —
    /// a flagged update, a reload — which the next flush binds.
    native_pipelines: Vec<u64>,
    /// What the document's content detection was last set to
    /// ([`Self::sync_content_detection`]), so a commit writes the document
    /// only when the registry's answer changed.
    content_detection: bool,
    /// Where committed frames leave for the painting side.
    outbox: ViewOutbox,
}

/// A name a registration change carries to
/// [`DocumentSlot::publish_route_changes`]: borrowed from a member call, or
/// the document's own `Arc<str>` ([`terminus::link::Document::take_pending_last_removals`],
/// [`terminus::link::Document::remove_listeners_of`]), which reaches the painter's set
/// without a second allocation.
trait RouteName {
    fn as_str(&self) -> &str;

    /// The name as the painter's set holds it.
    fn to_arc(&self) -> Arc<str>;
}

impl RouteName for &str {
    fn as_str(&self) -> &str {
        self
    }

    /// Allocates: a member call's name is only borrowed, so the one
    /// allocation is here, at the boundary, and only for a name sent.
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

    /// [`Self::document_mut`], shared.
    fn document_ref(&self) -> &LynxDocument {
        self.document.as_ref().expect(DOCUMENT_EXISTS)
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
        // document's observed-name changes say at the first commit.
        self.document = Some(document);
        Ok(())
    }

    /// Queues a `scroll` at every container an instant scroll request
    /// moved ([`terminus::link::Document::take_instant_scrolls`]), if anything in the
    /// document hears the name ([`Self::listens`]), and records the
    /// container as scrolling. Its `scrollend` comes from the painter, which
    /// carries the request out and then posts the container at rest — or,
    /// during a drag or a fling, once that ends — so nothing here ends a
    /// scroll.
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
    /// Not [`terminus::link::Document::hears`], which reads the observing *definitions*
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
    /// What that did to the name's registration count goes to the painter
    /// at once ([`Self::publish_route_changes`]), after the last removals
    /// freed nodes left waiting, so the painter is told whether a name is
    /// routed in the order its registration count crossed zero. A change of
    /// whether `node` has a [`EXPOSURE_RECORD_EVENT`] row is what registers
    /// it for exposure detection, or lets it go, and a change of whether it
    /// has a `uiappear` or `uidisappear` row whether it hears its element
    /// events ([`Exposure::set_registered`]); nothing else tells exposure
    /// about a listener. A change of whether `node` has a `layoutchange` or a
    /// `layout` row adds or removes its [`LayoutEventRegistration`]
    /// ([`Self::layout_registration_follows`]): `layoutchange` observes or
    /// unobserves it, `layout` turns a `text`'s layout report on or off.
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

    /// A realm's `listenerClosed`: `key` no longer listens for `name` on
    /// `node`, in any pass. As [`Self::open_listener`] for the registration
    /// change, for exposure and for the layout event registrations.
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

    /// Adds `registration` for `node` when the listener table now has a row
    /// for its event on `node` and had none (`before`), and removes it in the
    /// opposite case. A node that is not a live element is left alone.
    ///
    /// [`LayoutEventRegistration::ResizeObservation`] observes or unobserves
    /// the element ([`LayoutChangeObserver::observe`],
    /// [`LayoutChangeObserver::unobserve`]).
    /// [`LayoutEventRegistration::TextLayoutReport`] turns a `text` element's
    /// layout report on or off ([`terminus::link::Document::set_text_layout_reporting`]);
    /// any other element is left alone.
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

    /// Tells exposure what the listener table now says of `node`
    /// ([`exposure_rows`]), when that is no longer `before`.
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

    /// The realm `key` is closing or has been freed: every registration it
    /// filed leaves the table, so no later dispatch invites it and the one
    /// under way skips it, and each name whose last registration that
    /// removed is no longer routed by the painter unless an observed name
    /// keeps it. Idempotent: a child that closed itself is forgotten again,
    /// for nothing, when it is freed.
    ///
    /// The exposure registrations of its elements are not touched: they go
    /// with the elements, which the frame's `resetFrame` frees. Nor are its
    /// elements' [`LayoutEventRegistration`]s removed here: an element left
    /// observed with no `layoutchange`
    /// row is unobserved by the next delivery that reaches it
    /// ([`MainThreadRuntime::notify_resize_observers`]), and a `text` left
    /// reporting with no `layout` row has its report turned off by the next
    /// drain that reaches it ([`MainThreadRuntime::queue_text_layout_events`]).
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

    /// Applies the last removals the document's free path left waiting
    /// ([`terminus::link::Document::take_pending_last_removals`]): a node freed by a
    /// removal or a handle's collection takes its registrations with it, and
    /// no caller was there to hand the registration change to. One length
    /// check when no freed node held a registration.
    ///
    /// Drained before every registration change a member publishes, so the
    /// painter is told whether a name is routed in the order its
    /// registration count crossed zero, and before every commit is published
    /// ([`Self::sync_names`]) — the end of every entry, the epilogue's
    /// [`Self::commit_if_dirty`], included — so a last removal a collection
    /// made in an entry crosses before that entry's `ScriptFinished` does on
    /// the same outbox FIFO, as the realm's own removal did when the realm
    /// reported it.
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

    /// Publishes to the painter what `changes` — registration changes the
    /// listener table has already made, in the order it made them — move in
    /// what [`Self::routes`] says, one change at a time.
    ///
    /// The table is the only copy of which names are registered, so the
    /// state the painter was last told is rebuilt from it: a name with a
    /// change not yet published was registered before that change exactly
    /// when the change is a last removal. That is why every caller applies
    /// the waiting last removals first ([`Self::apply_pending_last_removals`]):
    /// then the table and what the painter was told differ by `changes`
    /// alone.
    ///
    /// A name the document handed out as its own `Arc<str>` reaches the
    /// painter as that `Arc`; one a member call only borrowed is allocated
    /// once, when it is sent ([`RouteName`]).
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

    /// Whether `name` was registered with the registration changes in
    /// `unapplied` not yet made: the first of them that names it decides,
    /// and with none the table does.
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

    /// Tells the painter whether `name` is routed when [`Self::routes`] for
    /// it differs between a registration state `before` and one `after`, the
    /// observed names unchanged. One registration change may move two names:
    /// `dblclick` is decided here from the `click`s the painter sends, so the
    /// first `dblclick` registration routes `click` there too, and the last
    /// one removed stops routing `click` unless `click` itself is wanted.
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

    /// Whether [`Self::routes_given`] for `name` is `after` now when it was
    /// not `before`: the routed state to tell the painter, or `None` when
    /// nothing moved.
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

    /// Applies the listener table's waiting last removals
    /// ([`Self::apply_pending_last_removals`]), and then the observed-name
    /// changes the document recorded since the last call, publishing each to
    /// the painter as [`Self::change_names`] says.
    ///
    /// Called before every commit is published ([`Self::flush`],
    /// [`Self::commit_if_dirty`]): the painter is told whether a name is
    /// routed in the mailbox the frame crosses in, ahead of the frame, so the
    /// painter routes a name from the first frame that shows the element
    /// observing it. One length check each when nothing was created or freed
    /// with a definition observing a name or a registration on it, and none
    /// before the document exists.
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

    /// Runs `change` on what decides [`Self::routes`] for `names` other than
    /// the listener table — [`Self::observed`], or the editing account —
    /// then tells the painter whatever moved in what [`Self::routes`] says,
    /// name by name in the order given: the painter's set is the union of
    /// the sides, so the painter routes a name from when any side first
    /// wants it until none does, and a realm registration for a name an
    /// element already observes moves nothing there. The `dblclick` pairing
    /// is [`Self::publish_route_change`]'s: a `dblclick` among `names` is
    /// followed by `click`.
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

    /// Whether the painter's name set must hold `name`: a realm
    /// registration or a live component element wants it
    /// ([`Self::listens`]), or, for `click`, a `dblclick`; or, on the editing
    /// account, it is a name of an open [`editing::EditingRoute`].
    fn routes(&self, name: &str) -> bool {
        self.routes_given(name, &|name| self.registered(name))
    }

    /// [`Self::routes`] with `registered` answering which names a realm
    /// registration holds. The editing account is read as it stands: a
    /// registration change never moves it.
    fn routes_given(&self, name: &str, registered: &impl Fn(&str) -> bool) -> bool {
        let wants = |name: &str| registered(name) || self.observed.contains(name);
        wants(name)
            || (name == CLICK_EVENT && wants(DOUBLE_CLICK_EVENT))
            || self.editing_routes(name)
    }

    /// Whether an [`editing::EditingRoute`] the editing account holds
    /// routes `name`.
    fn editing_routes(&self, name: &str) -> bool {
        editing::EditingRoute::ALL
            .into_iter()
            .any(|route| self.editing_routes & route.bit() != 0 && route.names().contains(&name))
    }

    /// Routes or stops routing one [`editing::EditingRoute`]'s names in the
    /// painter's name set. Published through [`Self::change_names`], after
    /// the waiting last removals ([`Self::apply_pending_last_removals`]), so
    /// each of its names moves only where [`Self::routes`] changes — a name
    /// a realm or a component wants anyway, or another route holds, is not
    /// touched — and in order with the listener table's own changes. Nothing
    /// at all when the route is already in that state.
    ///
    /// The change is a `watch` write ([`ViewOutbox::set_name_routed`]),
    /// with no acknowledgement and no wakeup, which the painter adopts at the
    /// start of its next routing pass ([`crate::Painter::dispatch_input`]
    /// polls the link first). So a route opened inside a job routes every
    /// event the painter decides after that job wrote it; one it decided
    /// before was filtered against the old set and never crossed.
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

    /// Moves the two routes the document's state decides, once per entry
    /// from the page's epilogue: [`editing::EditingRoute::Pointer`] while
    /// the document exists, [`editing::EditingRoute::Focus`] while an
    /// element is focused. One field read when nothing changed. The drag
    /// route is the press's own ([`MainThreadRuntime::set_pressed`]).
    fn sync_editing_routes(&mut self) {
        let (pointer, focused) = self.document.as_ref().map_or((false, false), |document| {
            (true, document.focused().is_some())
        });
        self.set_editing_route(editing::EditingRoute::Pointer, pointer);
        self.set_editing_route(editing::EditingRoute::Focus, focused);
    }

    /// An exposure attribute of the live element `node` was written or
    /// removed; see [`Exposure::attribute_changed`].
    fn exposure_attribute_changed(&mut self, node: terminus::link::NodeId) {
        let Self {
            document, exposure, ..
        } = self;
        exposure.attribute_changed(document.as_mut().expect(DOCUMENT_EXISTS), node);
    }

    /// The realm's `switchExposure` for the elements `nodes`; answers how
    /// many records it queued. See [`Exposure::switch`]. An empty list asks
    /// nothing of the document.
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
    ///
    /// Answers the commit id of the frame it published or holds, which a
    /// timed flush binds its pipelines to ([`pipelines`]).
    fn commit_flush(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<u64, String> {
        // The sheets first, then the metrics: a painter that binds while a
        // sheet is still in flight is adopted before the commit below, which
        // is then already at its size rather than held and recomputed.
        self.settle_sheets(thread)?;
        self.adopt_metrics();
        self.sync_names();
        self.sync_content_detection();
        let frame = self.document_mut().commit();
        let mut commit_id = frame.commit_id();
        if self.bound {
            // A frame held from before the binding is older than this one, and
            // publishing it after this one would put the stale frame back.
            self.held = None;
            self.outbox.publish_frame(frame);
            self.request_wanted_images();
            return Ok(commit_id);
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
            commit_id = frame.commit_id();
            self.outbox.publish_frame(frame);
        }
        self.request_wanted_images();
        Ok(commit_id)
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
            self.sync_content_detection();
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

    /// Whether the document holds anything the last commit did not apply.
    ///
    /// After the epilogue's [`Self::commit_if_dirty`] this is `true` only
    /// when that commit was skipped — a listed author sheet still
    /// outstanding — and `false` with no document yet.
    fn needs_render(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::needs_render)
    }

    /// Queues the `scroll` every instant scroll of this entry owes: a UI
    /// method's `scrollTo`, `scrollBy` or `scrollIntoView`, a component's
    /// own instant scroll, or the render's move to a new
    /// `scroll-initial-target` (an `x-swiper`'s `current`, a `viewpager`'s
    /// `select-index`). Asked by the page's epilogue after its commit, which
    /// is where that last one happens, so it joins the component events
    /// this entry produced; one length check when nothing scrolled. The
    /// `scrollend` follows from the painter's post of the container at
    /// rest.
    fn queue_instant_scroll_events(&mut self) {
        if self
            .document
            .as_ref()
            .is_some_and(LynxDocument::has_instant_scrolls)
        {
            self.instant_scrolls();
        }
    }

    /// Whether any producer has left a component event owing — the queue
    /// [`MainThreadRuntime::dispatch_component_events`] drains, asked once
    /// per entry.
    ///
    /// One `is_empty`, because nearly every entry's answer is no: an entry
    /// during which no source settled and no dialog closed posts nothing. A
    /// `load` handler that writes a `src` this document has already seen
    /// settle gets its outcome at the bind, and a `close` handler can close
    /// another dialog, so a delivery entry can leave the queue non-empty for
    /// an entry of its own.
    ///
    /// Two flags of the document's count as well: focus events a UI method
    /// left, and the document's selection having changed
    /// ([`terminus::link::Document::has_pending_selectionchange`]), whose W3C
    /// `selectionchange` that entry fires. Posting the entry is this
    /// engine's form of the Selection API's "queue a task", so every change
    /// before it runs owes one event.
    fn has_component_events(&self) -> bool {
        !self.component_events.is_empty()
            || self.document.as_ref().is_some_and(|document| {
                document.has_focus_events() || document.has_pending_selectionchange()
            })
    }

    /// Whether the commits since the last delivery left any
    /// `content-visibility: auto` skipping change owing — css-contain-2
    /// §4.4's queue, asked once per entry.
    ///
    /// One `is_empty` on a `Vec`, because nearly every entry's answer is no:
    /// a page whose `auto` boxes did not change state posts nothing.
    fn has_pending_content_visibility_changes(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_content_visibility_changes)
    }

    /// Runs the W3C "update intersection observations" steps
    /// ([§3.2.10](https://w3c.github.io/IntersectionObserver/#update-intersection-observations-algo))
    /// over the document's observers, and answers whether any of them has
    /// entries queued afterwards — the page's cue to post a delivery.
    ///
    /// Asked once per entry, after its commit, and nearly always answered at
    /// once: `terminus` runs nothing when there are no observers or nothing went
    /// stale since the last update that ran — a frame built, an offset
    /// moved, a target observed. It reads the last completed layout and the
    /// live scroll offsets and runs no pass, so an entry whose scroll was
    /// composed inside the encode window, and committed nothing, still
    /// updates against the offset it adopted.
    ///
    /// Every entry it queues is stamped with `time`, this slot's
    /// [`Self::timeline_milliseconds`], which the runtime reads and hands in
    /// before it borrows the slot: the reading the latest
    /// `vsync` or frame post handed this side, which is the
    /// `DOMHighResTimeStamp` that frame's animation-frame callbacks were
    /// called with and the one an image's `load` is stamped with. That is
    /// HTML's order — the observations are updated in the same rendering
    /// step as the callbacks, after them — so the entries' `time` compares
    /// directly with every other time this side reports, and nothing takes a
    /// second reading of the clock.
    fn update_intersection_observations(&mut self, time: f64) -> bool {
        self.document
            .as_mut()
            .is_some_and(|document| document.update_intersection_observations(time))
    }

    /// Whether exposure changes wait for a delivery entry — the page's
    /// cue to post one even when no observer has entries queued: a
    /// teardown, a `stopExposure` that sent records, or a freed element that
    /// was exposed queues its change outside any update.
    ///
    /// One `is_empty`, because nearly every entry's answer is no.
    fn has_pending_exposure(&self) -> bool {
        self.exposure.has_changes()
    }

    /// Runs `terminus`'s resize-observation update — the spec's "gather active
    /// observations" with the entries created
    /// ([`terminus::link::Document::update_resize_observations`]) — and answers whether
    /// it queued any entry: the page's cue to post the resize delivery.
    ///
    /// Asked once per entry, after its commit, and nearly always answered at
    /// once: `terminus` runs nothing when there are no observers, when no layout
    /// pass ran and nothing was observed since the last update that ran, or
    /// before the document's first layout. It reads the last completed layout
    /// and runs no pass.
    fn update_resize_observations(&mut self) -> bool {
        self.document
            .as_mut()
            .is_some_and(LynxDocument::update_resize_observations)
    }

    /// Whether any resize observer has entries queued — the page's cue to post
    /// the resize delivery when the update itself queued nothing new, for
    /// entries an earlier update left that no delivery has taken yet.
    fn has_pending_resize_notifications(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_resize_notifications)
    }

    /// Queues the `layout` event of every reporting `text` whose paragraph a
    /// committing layout run recomputed since the last call
    /// ([`terminus::link::Document::take_text_layout_reports`]), with the lines and the
    /// content box that commit left ([`ComponentEvent::TextLayout`]), in the
    /// order the runs first reached them. Asked by the page's epilogue after
    /// its commit, so the events join the component events the same entry
    /// posts; one `is_empty` when nothing reported.
    ///
    /// A node whose `layout` row left without the listener table telling
    /// this side — its realm was forgotten — has its report turned off here
    /// and queues nothing. A node whose paragraph has no committed lines (it
    /// stopped establishing one) queues nothing either.
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

    /// The `@font-face` rules the mounted sheets declared, once each.
    ///
    /// Empty before `createDocument`: there is no cascade yet, and the
    /// epilogue of the entry that mounts a sheet asks again.
    fn take_font_face_requests(&mut self) -> Vec<terminus::link::FontFaceRequest> {
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
    /// [`crate::realm::open_realm`] installed, with the realm's `ShadowRealm`
    /// children: the one table of the realms over this view's document.
    ///
    /// Shared, because a dispatch runs from two places ([`run_dispatch`]):
    /// the kernel's entries, which reach it here, and the synthetic dispatch
    /// member, which runs inside a JavaScript call of one of these realms —
    /// while an entry up the stack is still calling it — and reaches it
    /// through a weak handle ([`SharedRealms`]). So everything a call into a
    /// realm needs takes `&self`. This field holds the one strong handle,
    /// and dropping it is what frees the realm.
    core: Rc<RealmCore>,
    /// This realm's side of the workers it created, shared with the three
    /// host functions that drive them, and where a worker failure is reported
    /// from. It is also what finds the side of a child that created a
    /// worker, whose events go to that child.
    workers: Rc<super::workers::WorkerOwner>,
    slot: Rc<RefCell<DocumentSlot>>,
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
    last_click: Option<(terminus::link::NodeId, f64)>,
    /// What the pointer's editing default actions remember between events:
    /// the press a mouse drag extends, and the presses a quick second and
    /// third one count from.
    pointer_editing: editing::PointerEditing,
    /// The last [`terminus::link::editing::TextInputState`] published to the host; the
    /// default ("nothing editable is focused") until the first focus.
    text_input: terminus::link::editing::TextInputState,
    /// Where the host's typed text comes from, which decides what a
    /// `keydown`'s editing default action does ([`editing::key_command`]).
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
            realms: SharedRealms::default(),
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
                    .install_members(
                        engine,
                        startup_strings(&mut startup),
                        RealmId::Page.listener_key(),
                        None,
                        &RealmTiming::new(OpenerId::PAGE),
                    )
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
        // Shared from here on, with every realm's synthetic dispatch member
        // holding it weakly; the boot module, the realm's first JavaScript,
        // runs after this.
        let core = Rc::new(core);
        let _ = document.realms.set(Rc::downgrade(&core));
        Ok((
            Self {
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

    /// The realm and the two tables its core members write to, for the
    /// driver in [`crate::realm::owner`]: its epilogue, and the module loads
    /// and future settles that epilogue spawns.
    pub(crate) fn core(&self) -> &RealmCore {
        &self.core
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
            // A child that is gone took its workers with it; what one said
            // last reaches no realm.
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

    /// The page's half of an entry's epilogue, in its order, under one
    /// borrow of the document: the documents an `<svg content>` handed over,
    /// the editing routes, the commit, the instant scrolls' `scroll`s, the
    /// text-input state, the `text` `layout` events, and the resize and
    /// intersection updates. Answers which deliveries the entry left owing,
    /// for the page to post.
    ///
    /// - The documents first, asked of the host as one [`ViewNotice::RequestDocuments`] beside the
    ///   image sources the commit asks for; their reports come back as `ToMain::ImageEvents` and
    ///   apply through [`Self::apply_image_events`]. Nothing on this thread parses.
    /// - The editing routes before the commit, so the names an editing host needs routed are
    ///   published with the frame that shows the host.
    /// - An instant scroll this entry performed after the commit, which performs one of its own
    ///   when it honours a new `scroll-initial-target`: its `scroll` joins this entry's component
    ///   events, and its `scrollend` follows the painter's post of the container at rest.
    /// - The text-input state after it, so the geometry the host is handed is the frame's.
    /// - The `text` paragraphs it recomputed join this entry's component events as their `layout`
    ///   events, after it too.
    /// - The resize update before the intersection one, and each always: HTML gathers resize
    ///   observations before it updates intersection observations, and the intersection update must
    ///   run whether or not exposure changes are already waiting.
    /// - Component events are owed only once nothing is left uncommitted: an `<svg>`'s `load` reads
    ///   the element's layout box at delivery, which is stale until a commit applies the natural
    ///   size its report set, so while the commit above was skipped the whole batch is held.
    pub(crate) fn settle_entry(&mut self) -> Deliveries {
        let time = self.timeline_milliseconds();
        let mut slot = self.slot.borrow_mut();
        slot.request_documents();
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

    /// Adopts the painter's metrics, commits when anything is stale, and
    /// publishes once a painter has bound: [`DocumentSlot::commit_if_dirty`],
    /// the step of [`Self::settle_entry`] the benchmark harness and the
    /// tests run on its own. Never waits.
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

    /// Moves the document's animation clock to the painter's at the start of
    /// an entry; see [`terminus::link::Document::sync_animation_clock`].
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

    /// The newest reading of the view's timeline this side has been handed,
    /// in milliseconds ([`DocumentSlot::timeline_milliseconds`]): what an
    /// event this side originates is stamped with.
    fn timeline_milliseconds(&self) -> f64 {
        self.slot.borrow().timeline_milliseconds
    }

    /// Advances the animation timeline to the painting side's clock
    /// reading. Whether anything changed is the next commit's business.
    pub(crate) fn begin_frame(&mut self, now: f64) {
        // Seconds here, as the animation timeline wants them; milliseconds is
        // what an event's `timestamp` is in.
        let mut slot = self.slot.borrow_mut();
        slot.timeline_milliseconds = now * 1000.0;
        let _ = slot.document_mut().advance_animations(now);
    }

    /// Runs the realm's animation-frame callbacks at the painting side's
    /// vsync, through `bobcat:animation-frame`, the module every realm kind
    /// files them with.
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

    /// Hands one native module's answer to one function argument of a call
    /// this realm made back to it, through `bobcat:native-modules`.
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

    /// Writes the painter's scroll offsets into the document, which is how a
    /// user scroll reaches it. An offset inside the committed encode window
    /// commits nothing; one that has used half the window's headroom toward
    /// an edge ([`terminus::link::ScrollSlot::recenter_due`]) asks this entry's commit
    /// to bake windows re-centered on it, and one past the window already
    /// did inside `scroll_to`. The scroll-driven animations those containers
    /// drive then re-sample, which is a commit only when one moved, and an
    /// anchor-positioned box the offsets shifted out of its inset-modified
    /// containing block is laid out again to pick a position fallback
    /// ([`terminus::link::Document::redetermine_scrolled_fallbacks`]) — also a commit
    /// only then.
    ///
    /// A post naming an older request than the container's pending one
    /// ([`terminus::link::Document::pending_scroll_request`]) is dropped: the painter
    /// made it before it saw the request, so it would write back the offset
    /// the request replaced. One that names the pending request, or a newer
    /// one, is adopted and acknowledges it. A pending auto-scroll change
    /// ([`terminus::link::Document::auto_scroll`]) moved no offset, so no post is
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
        entries: impl Iterator<Item = (terminus::link::NodeId, crate::link::ScrollEntry)>,
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
    pub(crate) fn apply_image_events(&mut self, events: &[terminus::link::ImageEvent]) {
        let mut slot = self.slot.borrow_mut();
        let outcomes = slot.document_mut().apply_image_events(events);
        slot.component_events.extend_images(outcomes);
    }

    /// Whether the document holds anything the last commit did not apply:
    /// [`DocumentSlot::needs_render`], for the benchmark harness and the
    /// tests, which run the epilogue's steps one at a time.
    pub(crate) fn needs_render(&self) -> bool {
        self.slot.borrow().needs_render()
    }

    /// [`DocumentSlot::queue_instant_scroll_events`], for the benchmark
    /// harness and the tests.
    pub(crate) fn queue_instant_scroll_events(&mut self) {
        self.slot.borrow_mut().queue_instant_scroll_events();
    }

    /// [`DocumentSlot::has_component_events`], for the benchmark harness and
    /// the tests.
    pub(crate) fn has_component_events(&self) -> bool {
        self.slot.borrow().has_component_events()
    }

    /// The Selection API's `selectionchange`, when the document's selection
    /// changed since the last one ([`terminus::link::Document::take_selectionchange`]):
    /// one event per set flag, which is one task in the standard, fired at
    /// the document node, not bubbling and not cancelable (no event is
    /// cancelable here). The document node has no element, so no
    /// component's `handle_event` runs at it, and [`Self::dispatch`] builds
    /// no path when nothing hears the name; the text field's own
    /// `selectionchange` is the engine-only one at its editor
    /// (`runtime/editing.rs`), which this is not. A listener that moves the
    /// selection again sets the flag again, and the epilogue of the entry
    /// posts another, as the standard queues another task.
    ///
    /// Then queues the Lynx `selectionchange` derived from it at each `text`
    /// and `inline-text` whose record changed ([`report_selection_changes`]),
    /// read from the selection as the listeners left it, for
    /// [`Self::dispatch_component_events`] to deliver in this same entry.
    ///
    /// Returns what the listeners threw.
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

    /// The extras of a `scroll` or `scrollend` at `node`: Lynx's scroll
    /// metrics read now, the delta the post moved the container by, and
    /// `isDragging`, which this step reports false.
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

    /// Dispatches everything [`Self::apply_image_events`], the `image`,
    /// `svg` and `overlay` components, the dialog's UI methods, the scroll
    /// events ([`Self::adopt_scroll_offsets`],
    /// [`Self::queue_instant_scroll_events`]) and the text layout reports
    /// ([`Self::queue_text_layout_events`]) have queued, in the order they
    /// formed, and each connection and disconnection the `frame` component
    /// queued, as [`FRAME_CONNECTED_EVENT`] or [`FRAME_DISCONNECTED_EVENT`]
    /// at the frame, after which the children nothing needs any more are
    /// freed, outside every call, so the child a disposal retired goes in
    /// this entry.
    ///
    /// Before the queue, after the focus events a UI method left: the W3C
    /// `selectionchange` at the document node when the document's selection
    /// changed since the last one ([`terminus::link::Document::take_selectionchange`]),
    /// then the Lynx `selectionchange` at each `text` and `inline-text` whose
    /// record that changed, which the `text` module queues
    /// ([`report_selection_changes`]) for this same entry to deliver with the
    /// rest.
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
    /// component's element. A `text`'s `layout` is neither bubbling nor
    /// composed. An element freed between the event forming and
    /// this call resolves to nothing, exactly as a routed input event at a
    /// freed target does.
    ///
    /// Returns what the listeners threw. A handler that throws is nonfatal,
    /// the standing every event listener has here.
    pub(crate) fn dispatch_component_events(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let timestamp = self.timeline_milliseconds();
        // Focus events a UI method's `focus()` or `blur()` left in `terminus`
        // first: they formed inside the JavaScript call, before anything
        // queued after it, and the component events they produce (an
        // `x-input`'s `focus`) join the queue for a turn of their own.
        let mut failures = self.dispatch_focus_events(js_runtime, timestamp);
        failures.extend(self.dispatch_selectionchange(js_runtime, timestamp));
        let events = self.slot.borrow().component_events.take();
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
            // A detailed event carries the detail its component read when it
            // queued it — `<x-markdown>`'s strings, a text field's value and
            // selection — as extras borrowed from the queue for the call, one
            // per key the component had a value for.
            //
            // A `text`'s `layout` carries its lines as JSON text
            // ([`push_text_layout_extras`]), written here, before the extras
            // that borrow it.
            let lines_text: String;
            let mut extras: SmallVec<[(&'static str, HostArgument<'_>); 7]> = SmallVec::new();
            let (target, name, composed) = match *event {
                ComponentEvent::Image(terminus::link::ImageOutcome::Loaded {
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
                ComponentEvent::Image(terminus::link::ImageOutcome::Failed { node }) => {
                    if is_svg(self.slot.borrow_mut().document_mut(), node) {
                        continue;
                    }
                    (node, ERROR_EVENT, false)
                }
                ComponentEvent::Plain { node, name } => (node, name, false),
                // The realm holding the frame's handle has filed the internal
                // listener on it, so it alone is called, at any depth; a
                // frame freed since — its page reset, its handle collected —
                // hears nothing, as a routed event at a freed target does.
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
                    // Neither `scroll` nor `scrollend` bubbles, and neither
                    // is composed.
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
            // None bubbles. Every one but a detailed event is not composed
            // either, as HTML and web-core fire them: one queued at a UA
            // shadow tree's node stays inside that tree.
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
            // A disconnection lets go of the frame's card, and may have
            // retired a child the release frees here, outside every call,
            // rather than waiting for the epilogue.
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
    ///
    /// What the listeners threw is added to `failures`, as
    /// [`Self::dispatch`] adds it.
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
            failures,
        )
    }

    /// One `click` and, when it completes one, a `dblclick` (UI Events).
    ///
    /// The target is the nearest common inclusive ancestor of the down's
    /// target and the release's, along the parents a composed event path
    /// takes ([`terminus::link::Document::nearest_common_event_ancestor`]); the painting
    /// side has no tree to find it in. A second `click` at the same target
    /// within [`DOUBLE_CLICK_MILLISECONDS`] of the last one is then followed
    /// by a `dblclick` there, and a third starts over. The painting side sends
    /// a `click` when its name set holds `click`, which a realm `dblclick`
    /// registration routes as well ([`DocumentSlot::publish_route_change`]),
    /// so each is dispatched here only when a realm listener or a component
    /// hears it.
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
        // What a `click` listener threw does not keep the `dblclick` from
        // running, and both are reported, the `click`'s first.
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

    /// One whole dispatch, whatever produced it: the one place an event is
    /// delivered. The path is computed once, and one walk runs over it —
    /// the engine components' handlers at each step, and at the path's top
    /// step one call into each listening realm, which runs the realm's own
    /// Lynx walk over the steps of the path it holds handles for.
    ///
    /// A target freed since the event formed resolves to nothing rather than
    /// a path — a `NodeId` names one node for the life of the document, so
    /// the check is one lookup and can never hit a stranger — and so does a
    /// name nothing hears ([`terminus::link::Document::hears`]: no registration and no
    /// observing definition), which costs that one check and builds no path.
    ///
    /// **Which realms are called** is decided before the walk, from the
    /// document's listener table (`docs/tracking/deviations.md`): a realm is
    /// called when it holds a path registration for the name on a node of
    /// the path, in a pass that node's steps run
    /// ([`terminus::link::Document::listeners_on_path`]), or a `global-bindEvent`
    /// registration for the name the path reaches
    /// ([`terminus::link::Document::global_registrants_on_path`]): one whose node's
    /// tree root is a node of the path — the document node for the page's
    /// own and any light-tree node, which every connected composed path ends
    /// at; a frame's shadow root for a card's, so a card's global
    /// registration hears only the events whose path passes through its
    /// frame, as web-core's card listens on its own shadow root. web-core's
    /// document-level names ([`DOCUMENT_LEVEL_EVENTS`], `keydown` and
    /// `keyup`) are the exception: every realm with a global registration
    /// for one is invited ([`Invited`]). A realm with nothing to run is not
    /// called, so an event nothing on its path listens for costs no crossing
    /// at all. An invitee no longer invited when its call comes — a listener
    /// of a realm called before it freed its nodes, or closed it — is
    /// skipped then; a child already freed is skipped by
    /// [`RealmCore::call_module_export`] answering nothing. Each realm is
    /// named in the table by its [`RealmId::listener_key`] alone.
    ///
    /// **Where they are called** is the first capture step the walk
    /// reports, the path's top: the document node above the document element
    /// for every path in the document tree, a detached tree's root, or the
    /// shadow root a non-composed event inside a shadow tree ends at. The
    /// page realm is called first, then the children in the order they were
    /// constructed. A frame is a shadow tree like any other here: no record
    /// says which realm renders which nested page, and each realm finds its
    /// own steps in the path by the handles it holds. The walk is
    /// [`terminus::link::Document::advance`], one step at a time with the document
    /// borrowed for that step alone, so a realm's listeners may mutate the
    /// tree. A component's `stop_propagation` ends the walk after its step,
    /// so it keeps no realm from its call, every call being at the first
    /// step; a `stop_immediate_propagation` at that step, which reports no
    /// step, does. A realm's own `catch`
    /// and `stopPropagation` end that realm's walk and nothing else, as
    /// web-core's do (user ruling, 2026-10-18).
    ///
    /// **Each call** hands the realm the same argument list ([`encode_call`]),
    /// encoded once, right before the first call. Its path ([`encode_path`])
    /// is the standard's event path, the
    /// capture steps in target-first, root-last order, each with its
    /// shadow-retargeted target, less the steps whose node or target is no
    /// longer live and every shadow root, as the step count and then two
    /// numbers per step, the node's packed id and its target's, the way a
    /// touch event's points cross: the boundary takes primitives and
    /// structured clones only, and a clone can be minted by the realm alone.
    /// A realm skips the steps it holds no handle for: the page
    /// realm a UA component's shadow content, which script names no node of,
    /// and a frame's card; a card everything outside its nested page. Its
    /// `event.target` is the target of the first step it holds, so an event
    /// at shadow content reaches the page realm at its host, retargeted, as
    /// a browser reports it to every listener outside the shadow tree. A
    /// realm that holds no step at all — a card hearing a key at the
    /// document element — runs its `global-bindEvent` pass alone. The
    /// components' walk keeps every step: a component's shadow tree is its
    /// own.
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
    /// objects, in this order: the name, `bubbles` and the `timestamp`, then
    /// the path; the extras channel
    /// ([`encode_extras`]: the count, then name/value pairs), which is what a
    /// Lynx handler reads beside the W3C payload and which the realm makes
    /// the Lynx `detail` whenever there are any; then the W3C payload as
    /// [`encode_detail`] writes it, last because a touch event's points give
    /// it no fixed length. The kernel's events are W3C-shaped, and the realm
    /// is the Lynx layer that reshapes them (user ruling, 2026-10-13). No
    /// JSON is formatted here and none is parsed there.
    ///
    /// Each call is its own module-export call with its own microtask
    /// checkpoint, as a browser checkpoints after each listener invocation —
    /// except the exposure records', which [`Self::dispatch_checkpointed`]
    /// runs as one checkpoint per realm after the entry's last record.
    /// A call that throws is kept and the walk goes on, so every realm still
    /// hears the event; [`Self::finish_batch`] runs once, at the end; and
    /// every failure is added to `failures`, in the order the calls ran.
    ///
    /// No event is cancelable from script (the 2026-08-17 ruling stands,
    /// `docs/tracking/deviations.md`): no realm call answers anything about
    /// a default action, and the editing default actions
    /// (`main/runtime/editing.rs`) run whatever a listener did.
    ///
    /// Answers whether some realm published the export, which is all the host
    /// can know: nothing here says whether anything ran.
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

    /// [`Self::dispatch`], with its realm calls checkpointed as `checkpoint`
    /// says: each on its own, or none, for the caller to checkpoint once per
    /// realm after a run of dispatches.
    ///
    /// The kernel's entry into [`run_dispatch`], the one dispatch body, which
    /// the synthetic dispatch member enters too
    /// ([`install_synthetic_event_members`]). What is the kernel's is how a
    /// realm is called — `__BobcatDispatchEvent(name, bubbles, timestamp,
    /// stepCount, path…, extras…, detail…)` in the realm whose registrations
    /// are filed under the key ([`RealmId::from_listener_key`]), ending in
    /// that realm's checkpoint before the next realm is called, or with none
    /// for [`Checkpoint::Deferred`], every failure added to `failures` — and
    /// the [`Self::finish_batch`] that ends a dispatch that ran.
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
        // What this dispatch's own calls threw is what follows this mark in
        // the sink, which may already hold what the caller's earlier
        // dispatches threw.
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
        // Listeners remove elements too; the count they ran up is settled
        // here, at the end of the dispatch, as long as nothing failed — or by
        // the caller of a deferred run, after its checkpoints, since a
        // collection checkpoints too.
        if matches!(checkpoint, Checkpoint::Each)
            && let Err(error) = self.finish_batch(js_runtime, failures.len() == before)
        {
            failures.push(error);
        }
        called
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
    /// and the composed one at the host, [`terminus::link::event::EventPhase::AtTarget`]
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

    /// [`Self::dispatch_input_event`] for a test that is about the dispatch
    /// rather than about the payload: the event happened at `position`, with
    /// no wheel delta, no touch points and the time origin for a timestamp.
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

    /// Fires one `contentvisibilityautostatechange`
    /// ([css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event))
    /// per queued change, in frame order.
    ///
    /// The whole dispatch is `terminus`'s: it owns the path, the walk and the
    /// listeners, because the listeners are the engine's own components
    /// (`terminus::link::CustomElement` definitions — `<image>` today, `<list>` when it
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
        let timestamp = self.timeline_milliseconds();
        self.slot
            .borrow_mut()
            .document_mut()
            .dispatch_content_visibility_changes(timestamp);
    }

    /// [`DocumentSlot::update_intersection_observations`] at
    /// [`Self::timeline_milliseconds`], for the tests.
    #[cfg(test)]
    pub(crate) fn update_intersection_observations(&mut self) -> bool {
        let time = self.timeline_milliseconds();
        self.slot
            .borrow_mut()
            .update_intersection_observations(time)
    }

    /// [`DocumentSlot::has_pending_exposure`], for the tests.
    #[cfg(test)]
    pub(crate) fn has_pending_exposure(&self) -> bool {
        self.slot.borrow().has_pending_exposure()
    }

    /// Delivers every observer's queued entries to its handler, in creation
    /// order — "notify intersection observers"
    /// ([§3.2.5](https://w3c.github.io/IntersectionObserver/#notify-intersection-observers-algo)),
    /// which `terminus` runs whole, call included — and then the exposure
    /// changes that loop and the entries before it left, into the realm.
    ///
    /// Two kinds of observer exist today. An engine component's, a
    /// `terminus::link::CustomElement` definition's, carries a [`terminus::link::ElementHandler`]:
    /// `terminus` calls that element's `intersections_changed` hook in its own
    /// `[CEReactions]` scope, and drops the entries if the element is not a
    /// constructed component; **no realm is entered** for one. An exposure
    /// registration's carries the exposure handler, which only records
    /// changes: it cannot call the realm from inside `terminus`'s loop, because
    /// the realm is not the document's to lend, so the realm half runs here,
    /// after the loop, with the document released. The MTS
    /// `IntersectionObserver` binding's handler, once it exists, is the same
    /// shape: it queues `(observer, entries)` and this entry drains them after
    /// the loop.
    ///
    /// That half is web-core's order (`ExposureServices.ts:209-275`): each
    /// `uiappear`/`uidisappear` in the order the changes formed — one
    /// non-bubbling, non-composed dispatch at the element, with
    /// [`EventDetail::Exposure`] — then, in the same order, one
    /// [`EXPOSURE_RECORD_EVENT`] dispatch at the element of every change that
    /// has an `exposure-id`, non-bubbling and non-composed too, its id, scene
    /// and direction as extras. The realm holding the element's handle has
    /// the record listener's row, so it is the one called: the listener
    /// table answers which realm a record reaches, a detached element's
    /// included, whose path starts at itself. The realm's Lynx layer turns
    /// the records into the `exposure` and `disexposure` lists and orders
    /// them. An element freed since its change formed hears nothing, as a
    /// routed event at a freed target does, record included. The changes a
    /// `uiappear`/`uidisappear` listener queued — a `stopExposure`, an
    /// `exposure-id` written — are records only, and join this entry's
    /// records rather than waiting for the next delivery, so every record
    /// queued before the record dispatches begin reaches its realm in this
    /// entry's one checkpoint: a card let go of inside such a listener
    /// sends its last list in the flush that follows its stop.
    ///
    /// The record dispatches share one microtask checkpoint per realm called,
    /// run after the last of them ([`Checkpoint::Deferred`]), so what a
    /// record listener queues runs once every record of the entry is in:
    /// that is what lets the realm send one list of each kind per entry, as
    /// web-core sends one per batch.
    ///
    /// A hook or a listener may mutate the tree. Whatever it wrote is
    /// committed by the delivery entry's own epilogue, whose update then runs
    /// again against the new frame.
    ///
    /// Returns what failed: a listener that threw. Each is nonfatal and
    /// reported as `ListenerFailed`; the rest is still delivered.
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
            // web-core's `bubbles: false, composed: false`
            // (`ExposureServices.ts:210-218`).
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
        // What the record dispatches and their checkpoints throw follows
        // this mark; an appear or disappear listener that threw does not
        // keep the batch below from being settled.
        let records_from = failures.len();
        // What those listeners queued: records only, since only an
        // observer's report carries an element event.
        let late = self.slot.borrow().exposure.take_changes();
        debug_assert!(late.iter().all(|change| !change.element_event));
        changes.extend(late);
        // One checkpoint per realm after every record, rather than one per
        // call: what a record listener queues runs once all of the entry's
        // records are in, so the realm sends one `exposure` and one
        // `disexposure` list.
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

    /// [`DocumentSlot::update_resize_observations`], for the tests.
    #[cfg(test)]
    pub(crate) fn update_resize_observations(&mut self) -> bool {
        self.slot.borrow_mut().update_resize_observations()
    }

    /// [`DocumentSlot::has_pending_resize_notifications`], for the tests.
    #[cfg(test)]
    pub(crate) fn has_pending_resize_notifications(&self) -> bool {
        self.slot.borrow().has_pending_resize_notifications()
    }

    /// Delivers every resize observer's queued entries to its handler, in
    /// creation order — the broadcast of resize-observer-1 §3.4.5, which
    /// `terminus` runs whole ([`terminus::link::Document::notify_resize_observers`]) — and
    /// then dispatches `layoutchange` at every target the document's
    /// `layoutchange` observer delivered, in delivery order
    /// (`crate::main::layout_change`).
    ///
    /// The observer's handler only records the targets: it cannot call the
    /// realm from inside `terminus`'s loop, so the realm half runs here, after the
    /// loop, with the document released, as
    /// [`Self::notify_intersection_observers`] runs exposure.
    ///
    /// An overlay's observed box is its shadow `#dialog`'s (web-core's
    /// `layoutChangeTarget`, [`crate::main::tree::layout_change_target`]):
    /// its `layoutchange` is dispatched at the overlay with the dialog's rect.
    ///
    /// Each target is one dispatch, not bubbling and not composed, web-core's
    /// `commonComponentEventSetting` (`CommonEventsAndMethods.ts:23-58`),
    /// with four extras the realm's Lynx layer builds the detail
    /// `{id, dataset, width, height, left, top, right, bottom}` from: `left`,
    /// `top`, `width` and `height` of [`terminus::link::Document::bounding_client_rect`]
    /// read here, in the Lynx view's CSS px with nothing subtracted (user
    /// rulings 3 and 8 of 2026-10-20), or all four `0` when the target has no
    /// box any more. A target freed since its entry formed hears nothing, as
    /// a routed event at a freed target does; one whose last `layoutchange`
    /// row left without the listener table telling this side — its realm was
    /// forgotten — is unobserved here and hears nothing either.
    ///
    /// A listener may mutate the tree. Whatever it wrote is committed by the
    /// delivery entry's own epilogue, whose update runs again against it.
    ///
    /// Returns what failed: a listener that threw. Each is nonfatal and
    /// reported as `ListenerFailed`; the rest is still delivered.
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
                // A listener earlier in this loop may have freed it.
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

    /// [`DocumentSlot::queue_text_layout_events`], for the tests.
    #[cfg(test)]
    pub(crate) fn queue_text_layout_events(&mut self) {
        self.slot.borrow_mut().queue_text_layout_events();
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
        let own = run_due_timers(&self.core.engine, js_runtime, &self.core.timers);
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

    /// The `@font-face` rules the sheets mounted so far declared and this
    /// realm has not reported before. Empty until a sheet is mounted.
    pub(crate) fn take_font_face_requests(&mut self) -> Vec<terminus::link::FontFaceRequest> {
        self.slot.borrow_mut().take_font_face_requests()
    }

    /// Files one loaded `@font-face` source under its declared family.
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
/// `createPage` and the same `flushElementTree`. Which page a realm renders
/// is the realm's own record, kept nowhere here: a child's
/// `bobcat:element` answers `__CreatePage` with the nested page
/// `bobcat:frame-boot` handed it (`__BobcatUsePageRoot`), and its card's
/// sheet goes to the frame's shadow root because `adoptStyleSheet` was told
/// so. What a child differs in is the registrant its event registrations are
/// filed under (its [`RealmId::listener_key`]), its `retired` flag, which its
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
    /// The page realm's core, which every realm's synthetic dispatch member
    /// calls the invited realms through.
    realms: SharedRealms,
}

/// The page realm's [`RealmCore`] — its engine and its `ShadowRealm`
/// children, the one table of the realms over a view's document — as the
/// host member `dispatchEvent` reaches it from inside a JavaScript call
/// ([`install_synthetic_event_members`]).
///
/// Set once, by [`MainThreadRuntime::new`], as soon as the core exists:
/// the members are installed while it is being built, and no JavaScript of
/// the realm runs before it is set. Weak, because the members holding it
/// live in the realms the core holds, which a strong handle would never let
/// go; it answers nothing once the core has dropped, with the view.
type SharedRealms = Rc<OnceCell<Weak<RealmCore>>>;

impl DocumentHost {
    /// Installs the document members into one realm, with `startup` as its
    /// one-shot strings, `key` as the registrant its event registrations are
    /// filed under — the realm's [`RealmId::listener_key`] — and `retired`
    /// as the flag its `closeRealm` sets: `None` for the page realm, which
    /// cannot close itself.
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
    /// A child's members: the page realm's, with no startup strings, which
    /// are the page's, its event registrations filed under its
    /// [`RealmId::listener_key`], and a `closeRealm` that retires the child;
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
        let created = ClockInstant::now();
        let listener_key = RealmId::Child(key).listener_key();
        // The card's realm is its own opener of pipelines, as a nested
        // card is its own `LynxTemplateRender` in native.
        let timing = RealmTiming::new(self.outbox.registry().allocate_opener());
        self.install_members(
            engine,
            STARTUP_STRINGS.map(|name| (name, None)),
            listener_key,
            Some(retired),
            &timing,
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
        timing.set_created(created, ClockInstant::now());
        Ok(Box::new(ChildDocumentState {
            workers: Box::new(workers),
            slot: Rc::clone(&self.slot),
            key: listener_key,
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

/// [`install`] for a member that may be called again while a call of it is
/// running ([`ScriptEngine::register_reentrant_host_module_function`]).
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
    // its `closeRealm` lets go of.
    key: ListenerKey,
    // A child's own release flag, which its `closeRealm` sets; `None` in the
    // page realm.
    retired: Option<Rc<Cell<bool>>>,
    // The realm's own timing state, which its flushes bind as.
    timing: &Rc<RealmTiming>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        fn createPage() |document| {
            Ok(node_id_value(document.document_element().id()))
        }
        fn createElement(tag: string_argument) |document| {
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

    // `(pipelineId, timingFlag)`: the flush's `pipelineOptions.pipelineID`
    // (0 for none) and its `__lynx_timing_flag` option (`""` for none).
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

/// Installs the three members a nested card's realms speak to, all generic:
/// `framePage` reads a `frame`'s nested page, `resetFrame` frees it with the
/// card under it and builds the next card's, and
/// `closeRealm` retires the realm that calls it and lets go of every
/// registration it filed. Which realm renders which frame, and when one is
/// let go of, is `bobcat:frame`'s to decide; the host keeps no record of it.
fn install_nested_page_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    key: ListenerKey,
    retired: Option<Rc<Cell<bool>>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        // The nested page a `frame` holds now, which belongs to the card
        // rendered under it: `resetFrame` frees it with that card and builds
        // the next card's. Refused past `MAX_FRAME_DEPTH` frames, so a card
        // that embeds itself stops there. Nothing is recorded.
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

        // Lets go of the card a frame shows: frees its nested page with
        // everything under it and the sheets on the frame's shadow root,
        // and builds the next card a page of its own
        // ([`crate::main::tree::reset_frame`]). The old card's listener rows
        // and exposure registrations leave with its nodes, through the
        // document's free path; its realm is the old card's own business,
        // and no dispatch calls it for a name it holds nothing for.
        fn resetFrame(frame: node_id_argument) |document| {
            validate_live_element(document, NAME, frame)?;
            reset_frame(document, frame)
                .ok_or_else(|| format!("{NAME} requires a frame element"))?;
            Ok(HostValue::Undefined)
        }
    }

    // Retires the child, and lets go of every registration it filed now
    // rather than when the release step frees it: from here on no dispatch
    // calls it, and a name only it held is no longer routed by the painter.
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
        // The answer is the document node's id, the target of the document's
        // own events (the W3C `selectionchange`), which the realm's
        // `Document` is the handle of. The page id is not: `createPage` is
        // still what hands the realm the permanent root, and it now has a
        // document to read it from.
        let mut slot = borrow_slot(NAME, &tree)?;
        slot.create_document(config)?;
        Ok(node_id_value(slot.document_ref().root_node().id()))
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
/// crosses is one call per change of what the realm has registered for a
/// name on an element, which
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

/// Installs the two members a realm's `__CreateEvent` and `__DispatchEvent`
/// are written over.
///
/// `timelineMilliseconds()` answers the newest reading of the view's
/// timeline this side was handed ([`DocumentSlot::timeline_milliseconds`]),
/// which `__CreateEvent` stamps the event it builds with: native stamps an
/// event when it is created, as DOM does, and the realm has no clock on this
/// timeline of its own.
///
/// `dispatchEvent(node, name, bubbles, composed, timestamp, detail)` is one
/// synthetic dispatch at the element `node`, made by the realm filed under
/// `own`, and it is the routed dispatch: the one body
/// [`MainThreadRuntime::dispatch`] runs ([`run_dispatch`]), over the page
/// realm's [`RealmCore`] — `realms`, the one table of the realms over the
/// document — with the same path, the same invitation rule, the same order
/// and the same realm calls. The components' walk hears an
/// [`EventDetail::None`], since the script `detail` is a realm value; every
/// realm the listener table invites is called at the path's top step, the
/// page realm first and then the children in construction order, before the
/// components on the steps below it. Each other realm is handed `detail` as
/// the structured clone the dispatching realm's call wrote for it
/// ([`CallDetail::Cloned`]); the dispatching realm itself is called like any
/// other, in its place in that order, and handed [`CallDetail::Dispatched`],
/// which tells it to walk the event object its `__DispatchEvent` kept for
/// the call, so the object keeps its identity for its listeners. A realm the
/// table does not invite is not called, the dispatching one included. The
/// member answers `undefined`; a node that is not a live element, and
/// arguments that are not its shape, throw.
///
/// What differs from the kernel's entry is the realm call alone. The member
/// runs inside a JavaScript call, so no call checkpoints
/// ([`RealmCore::call_module_export_nested`]): the enclosing operation's
/// checkpoint drains the runtime-wide queue, and a rejection a callee left
/// is the callee's. What a call throws is that realm's listener failure,
/// sent as [`crate::view::EngineEvent::ListenerFailed`] on the view's
/// engine-event channel, as a routed dispatch's is reported; it is never
/// thrown into the dispatching realm, and the next realm is called all the
/// same. Nothing ends the batch here either: the enclosing entry's
/// [`MainThreadRuntime::finish_batch`] settles the removals. A component's
/// `stop_propagation` ends the components' walk after its step, and an event
/// a component queues during it is delivered by the component-events entry
/// the epilogue posts, as one queued inside any other member is.
///
/// Registered re-entrant: a listener may dispatch an event of its own,
/// synchronously, as DOM allows, and that dispatch can call any realm whose
/// call is still running, this member's own included. The body borrows the
/// slot per use and the realms only shared, and releases the slot before
/// any realm is called.
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
        // Set before the realm's first JavaScript, and gone only with the
        // view, while no JavaScript runs.
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

/// Whether a change of `name`'s registrations on an element can change what
/// exposure is told of it ([`exposure_rows`]): the record event, and the two
/// an element hears its exposure by.
fn is_exposure_event(name: &str) -> bool {
    name == EXPOSURE_RECORD_EVENT || name == APPEAR_EVENT || name == DISAPPEAR_EVENT
}

/// What the listener table says of `node` for exposure: whether some realm
/// filed the record listener ([`EXPOSURE_RECORD_EVENT`]) on it, which is what
/// registers it, and whether it hears its element events
/// ([`hears_exposure`]).
fn exposure_rows(document: &LynxDocument, node: terminus::link::NodeId) -> (bool, bool) {
    (
        document.has_listener(node, EXPOSURE_RECORD_EVENT),
        hears_exposure(document, node),
    )
}

/// Whether some realm registered for `uiappear` or `uidisappear` on `node`,
/// in any form: what makes a registered element hear its element events.
/// Every form counts, `__AddEventListener`'s
/// included — native's rule: `FiberAddEventListener` syncs the name into the
/// element's event set for every form it files
/// (`core/runtime/lepus/bindings/renderer_functions.cc:1077`), and native
/// exposure fires `uiappear` at an element whose event set has it
/// (`UIExposure.java:543-547`). web-core marks an element from `__AddEvent`
/// alone (`createElementAPI.ts:160-170`).
fn hears_exposure(document: &LynxDocument, node: terminus::link::NodeId) -> bool {
    document.has_listener(node, APPEAR_EVENT) || document.has_listener(node, DISAPPEAR_EVENT)
}

/// What a child's host modules keep for it: its `Worker` owner, and the
/// release of its event registrations, which [`Drop`] runs when the child is
/// freed — so a child freed without closing itself (a `ShadowRealm` nothing
/// referred to any more) leaves no row in the listener table, as one that
/// called `closeRealm` already left none.
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
        // (`terminus::link::CustomElement::invoke`, run by the kind's component in
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
        // (`terminus::link::html::dialog`). On `NotFound` the shared base set every
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
            let value = element.attribute(name).map(std::borrow::Cow::into_owned);
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
            for name in element.attribute_names() {
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
/// them, and what they drive — the observer of a registered element, re-armed
/// from them — sits beside the document in the slot, out of a component's
/// reach. They register nothing by themselves: the record listener the
/// realm's Lynx layer files does. The slot is told after the mutation, so the
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
        } else if name == TIMING_FLAG_ATTRIBUTE && !value.is_empty() {
            // Lynx's timing flag: bound to the next flush's commit.
            handle.pending_flags.push(Arc::from(value));
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
/// `switchExposure(on, sendEvent, ids)` is `lynx.stopExposure` /
/// `lynx.resumeExposure` as native runs them for one card: for the elements
/// `ids` names, the ones the calling realm registered, which only it knows
/// ([`Exposure::switch`]). Both flags are `0` or `1`, as every flag a tree
/// member takes; `ids` is comma-joined decimal node ids, as
/// `childElementIds` carries a list, `""` for none, and an id that names no
/// registered element is skipped. It answers the number of records it
/// queued, which the next intersection delivery entry dispatches at their
/// elements: one per listed element exposed under an `exposure-id` for a
/// stop with `sendEvent`, 0 otherwise. Whether an element is registered for
/// its `uiappear`/`uidisappear` is not a member's: the listener table answers
/// it ([`DocumentSlot::open_listener`]).
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
fn packed_node_id(node: terminus::link::NodeId) -> f64 {
    node.to_bits() as f64
}

/// The same handle as a value a host callback returns, rather than as an
/// argument the runtime lends into a call.
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

/// A list of node ids as the realm writes one: the packed ids in decimal,
/// joined with commas — the shape `childElementIds` answers a list in — and
/// `""` for none. Each id must have a handle's shape; whether it names a live
/// element is the caller's to ask.
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

/// A flag the realm spells as `0` or `1`, which is every boolean a tree
/// member takes.
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

/// The passes one registration runs in, as `listenerOpened` takes them: the
/// bits of [`ListenerKinds`], at least one. An empty set is not a registration —
/// `listenerClosed` is the removal — so 0 is refused with every non-integer
/// and every bit outside the three.
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

/// A finite JavaScript number: a synthetic event's timestamp, which
/// `__CreateEvent` read off `timelineMilliseconds`.
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

/// A JavaScript boolean, which is what the boot module hands `createDocument`
/// its five switches as, and a synthetic event its two flags.
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
