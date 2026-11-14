# bobcat-core: Events, observers, exposure and layout events

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

An element handle is an `EventTarget`, and the registration half lives in
`packages/bobcat-element`: listener closures live only in the realm and nothing
about a handler ever crosses into Rust. The parts of `__AddEventListener` that
duplicate `__AddEvent` are deliberately absent: `closure_type` selecting a
handler string and `bind_type` selecting Lynx's `catch` forms are not honored.

`__AddEvent` is the other registration form, and the one ReactLynx's compiled
output uses for every `bind*`/`catch*` prop: it files handlers under a Lynx
dispatch form — `bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
`global-bindEvent`. **Two handler kinds, filed apart**: a *string* is an opaque
background-thread handler name, an *object* is a worklet, each element holds
one of each per event name, and a call of one kind never disturbs the other, so
a `main-thread:bindtap` and a `bindtap` on the same element both run (native
Lynx's `static_events_` beside `lepus_events_`; web-core's cross-thread-handler
map beside its run-worklet map). Only a nullish handler clears, and it clears
both kinds. Within a kind the key is the event *name* alone, the dispatch form
carried inside the entry, so filing `catchtap` over `bindtap` of the same kind
replaces it, form included. A worklet runs through the card's own `runWorklet`;
a string is published with a snapshot of the event through the MTS runtime. A
`catch` form ends the walk before either kind is delivered. Anything else
non-nullish is ignored, which is web-core's behavior. `global-bindEvent` is
filed in its own slot and delivered in a pass of its own after the two path
passes, not a pass over the path: every element holding a global registration
for the name is delivered to, in registration order, whatever path the event
took and whether or not a `catch` ended the walk, so a global delivery has
`eventPhase` `NONE`.

The kernel walks; each realm runs its Lynx walk over the steps it holds
(scheme D, user ruling 2026-10-18; every realm at the path's top, user ruling
2026-10-19). The host computes the event path while it holds the document and
walks it once (`terminus`'s `EventWalk`), and at the path's first capture step
releases the document and makes one call to each listening realm's Element
module `__BobcatDispatchEvent` export, the page realm first, then the
children in construction order — the page realm's through
`quickjs::ScriptEngine::call_module_export`, a `frame`'s card's through
`ChildRealms::call_module_export` — every call carrying the same path,
encoded once before the first (`encode_path`): the standard's bubble steps,
target-first, as two comma-joined decimal id strings — the nodes, and
position for position each step's shadow-retargeted target, less the steps
whose node or target is no longer live and the shadow roots. A frame is a
plain shadow tree to the dispatch. Each realm skips the steps it holds no
handle for — the page realm a UA shadow tree's content, which script names
none of, and every card; a card everything outside its nested page — and its
`event.target` is the target of the first step it holds, so an event at
shadow chrome reaches the page realm at its host (routed input events are
composed, the components' events are not, as HTML and web-core fire them);
a realm holding no step runs its `global-bindEvent` pass alone, aimed at each
registering element. Then the name and primitives alone: `bubbles`, the
`timestamp` (milliseconds on the view's timeline); the extras channel
(`extraCount`, then name/value pairs: what a Lynx handler reads beside the W3C
payload, such as a `load`'s size or a `scroll`'s metrics, which the realm makes
the Lynx `detail`); then the W3C payload's kind and values — for an input
event the position, the wheel delta, a pointer's `button`, `buttons`,
`pointerId` and `pointerType` (each `undefined` where the shape has none), and
four numbers per touch point (`identifier`, `x`, `y`, flags); for a key event
its `key` and `code` strings, `repeat` and a modifier bitmask; for
`beforeinput`/`input` the `inputType`, the `data` (or null) and `isComposing`;
for a composition event its `data`; for a focus event the `relatedTarget`'s
id (or null). Kernel events
are W3C-shaped and `element-papi.ts` is the Lynx layer that reshapes them
(user ruling 2026-10-13; `docs/tracking/dom-events.md`). The realm builds
the `detail` object, the W3C members on the event object and the three touch
lists out of them; no JSON is formatted on the host side and none is parsed
in the realm. One call
is the realm's whole part of one dispatch, so one event object serves it. The
realm runs the capture, bubble and `global-bindEvent` passes, derives
`eventPhase` per step, and ends its walk itself; neither `stopPropagation` nor
`stopImmediatePropagation` crosses the boundary, so a realm's `catch` ends its
own walk and no other realm's. In a `frame`'s card `x`/`y` are relative to
the frame's border box (`boundingClientRect` on the frame, once per position
dispatch); `clientX`/`clientY`/`pageX`/`pageY` stay the viewport's.

Which realms are called is the host's, from `terminus`'s listener table, decided
before the walk: a realm with a path registration for the name on the path
in a pass its steps run (`Document::listeners_on_path`), or with a
`global-bindEvent` for it that the path reaches
(`Document::global_registrants_on_path`: the registering node's tree root —
the document node for a light-tree node, a frame's shadow root for a card's
node — is a node of the path, so a card's global registration hears only the
events whose path passes through its frame, as web-core's card listens on its
own shadow root); and, for web-core's document-level names
(`DOCUMENT_LEVEL_EVENTS`: `keydown`, `keyup`, `WASMJSBinding.ts:30`), with a
`global-bindEvent` for it anywhere, so a card hears every key. A realm with nothing to run is
not called; an event nothing on its path listens for calls no realm at all;
a realm no longer invited when its call comes (the same three queries, read
again after each call that ran) is skipped then. For a
path in the document tree the page realm runs exactly the steps the one call
before the table carried. Each call checkpoints on its own; a call that throws is
collected into the caller's failure list and the walk goes on. The realm reports per
(element, name) registrations with their passes: `listenerOpened(node, name,
kinds)` (1 bubble, 2 capture, 4 global, or'ed; the host replaces the kinds on
a repeat) and `listenerClosed(node, name)`, from `element-papi.ts`'s
`reconcile` only; a freed node's rows leave in the document's free path, so a
collected handle tells the host nothing.
Nothing crosses back: no event is cancelable from script. The realm's event
object has no `preventDefault`, `cancelable` or `defaultPrevented` (the
2026-08-17 ruling stands, `docs/tracking/deviations.md`: a Lynx event object
has none, and web-core's root listeners are `passive: true`), the editing
events are dispatched not cancelable, and the editing default actions run
whatever a listener did. The one veto on a built-in behavior is the embedder's
`InputEvent::default_prevented`, answered by the input router.

Editable text (`docs/editable-text-design.md`, E2) is wired here.
`LynxView::edit` sends `ToMain::Edit`; `MainThreadRuntime::apply_edit`
prepares the command in `terminus`, dispatches the events before the change at the
host, commits, and dispatches the rest. A routed `keydown`/`keyup` is
delivered at `Document::focused()` when there is one, and a `keydown` at a
focused host runs the keyboard's editing command (`key_command`), which
inserts the text the key typed (`InputKind::Key`'s `text`, carried in
`EventDetail::Key` and never crossing into the realm), not its `key` — unless
the host set the view to `editing::TextInputSource::Commands`
(`LynxView::set_text_input_source`, `ToMain::TextInputSource`; the browser
host does, since its `<textarea>` proxy sends every insertion, deletion and
line break as an edit), when a `keydown` only moves the selection and
selects all; a mouse's primary `pointerdown`, or a finger's or pen's tap (the
`click`), inside an editing host focuses it
(dispatching `focus`/`focusin`, and `blur`/`focusout` at the host that lost
it — drained from `terminus` right after the press) and places the caret, a second
and third quick press select the word and the line, and a held mouse drag
extends. The slot routes those names on an editing account of its own, by
state (`editing::EditingRoute`), one more term of `DocumentSlot::routes_given`
beside the listener table and the observed names, whose changes go out through
`DocumentSlot::change_names` after the waiting last removals: `pointerdown`
and `click` from the page's first epilogue on (a press selects any text whose
used `user-select` is not `none`, and a press outside everything blurs and
clears), `keydown` while an element is focused (both from the page's epilogue,
`DocumentSlot::sync_editing_routes`),
and `pointermove`/`pointerup`/`pointercancel` only while a mouse drag is held
(`MainThreadRuntime::set_pressed`, inside the press's job). The kernel is not
a registrant in the listener table, so an event routed only on that account
invites no realm and builds no path; `deliver_input_event` runs the default
action whenever the account routes the name. The epilogue publishes the
focused host's `TextInputState` after its commit when it changed. An editing host inside a UA shadow tree (a
text field's editor, E5) dispatches its edit and focus events non-composed, so
they stay in that tree for its component; a press in a text field outside its
editor focuses the editor (`tree::focus_delegate`); after every applied
command `apply_edit` fires the engine-only `selectionchange` at the host
through the Rust walk; and focus events a UI method's `focus()`/`blur()` left
in `terminus` are dispatched from the entry that delivers component events. That
entry also fires the W3C `selectionchange` at the document node (not
bubbling) whenever `terminus`'s flag is set, which `has_component_events` asks so
the epilogue posts the entry; the realm's boot-module `document` is the
document node's handle and its `addEventListener` hears it. The `text`
component's Lynx `selectionchange` (`{start, end, direction}` at `text` and
`inline-text`, web-core's) is derived from it in the same entry
(`tree::text::report_selection_changes`).

That host side is one function, `MainThreadRuntime::dispatch`, the only place
an event script hears is delivered. It computes the steps once and drives
`terminus`'s walk over them (`Document::event_walk`/`Document::advance`) step by
step, so an engine component whose definition lists the type in
`CustomElement::observed_events` hears it at its own step; a component's
`stop_propagation` is the walk's one stop flag and ends the walk after its
step, which keeps no realm from its call, every call being at the first. What
the event carries
is `terminus::link::event::EventDetail` (one variant per payload shape), which the router
fills in, `ToMain::DispatchEvent` carries, the Rust walk lends out and
`encode_detail` turns into the realm's numbers. A name a component observes is
in the painter's name set while an element of that component exists, beside
the listener table's names: `terminus`'s `Document::take_observed_name_changes`
reports each observed name that became live or stopped being, and
`Document::take_pending_last_removals` each name whose last registration a
freed node removed, both drained before each commit is published
(`DocumentSlot::sync_names`).
CSSOM-View's `scroll` and `scrollend` are decided on this thread, never on
the painter: `adopt_scroll_offsets` queues a `scroll` per container a post
moved and a `scrollend` when a post names it at rest after it scrolled, and an
instant scroll request — a script's, a component's, or the render's move to a
new `scroll-initial-target` (`terminus::link::Document::take_instant_scrolls`) — queues
its `scroll` from its entry's epilogue, after the commit, and gets its
`scrollend` from the painter's rest post after carrying the request out. A wheel gesture rests
once, 100 ms after its last tick (`WHEEL_REST_SECONDS`). All are component
events, non-bubbling, with Lynx's scroll metrics as extras, queued only for a name a realm
listener or a component hears at that moment; which containers are scrolling
is tracked regardless.

css-contain-2 §4.4's `contentvisibilityautostatechange` takes the same
dispatch, from an entry of its own. The commit that determines
`content-visibility: auto` relevance leaves the elements whose *skipping*
changed in `terminus`'s own queue; the page's epilogue posts **one fresh entry**
for the batch, which is the spec's "posting a task" — the event is never
delivered inside the entry that committed — and that entry calls
`MainThreadRuntime::dispatch_content_visibility_changes`, which fires each one
through `MainThreadRuntime::dispatch`: components observing the name hear it,
and so does every realm registered on the path, a card's own
`addEventListener` included (user ruling 2026-10-27, revoking the 2026-09-21
one that kept it from script). `skipped` is the event's W3C member and the
Lynx `detail` is `{}`. `bubbles = true` is a recorded choice where the spec is
silent (Chromium bubbles, WebKit and Gecko do not); `composed = false`; nothing
cancelable. The painting
side routes against the *name* set, and the painter is told whether a name
is routed only when the name's document-wide registration count crosses
zero — its first registration anywhere added and its last removed — which
the listener table's `RegistrationChange::{FirstAdded, LastRemoved}` are
(`DocumentSlot::publish_route_changes`, `ViewOutbox::set_name_routed`).

A second engine-decided path is the **W3C IntersectionObserver primitive**
(`crates/terminus/src/visual/intersection/`, user-directed 2026-10-08, Rust-only:
no `IntersectionObserver` global and no `lynx.createIntersectionObserver`
yet). The page's epilogue runs `Document::update_intersection_observations`
right after its commit — whether or not that commit built a frame, because a
scroll adopted inside the encode window commits nothing and still moves every
answer — and, when a registration crossed a threshold, posts one fresh entry
whose job is `Document::notify_intersection_observers`, §3.2.5's whole loop:
each observer's boxed `IntersectionEventHandler` is called in creation order
(`dyn`, user ruling 2026-10-10, for simplicity). An engine component's
observer carries `ElementHandler(node)`, which calls its
`CustomElement::intersections_changed` hook, one reaction scope per call, and
is bound to the element; the MTS binding's handler will queue
`(observer, entries)` on a runtime queue the same entry drains into the realm
after the loop. The update is the spec's §3.2.10 pass — threshold index and
`isIntersecting` compared with the registration's previous pair — over
transform-aware, clip-aware geometry; see `docs/terminus-architecture.md`
"Intersection observations".

**Lynx exposure** (`crates/bobcat-core/src/main/exposure.rs`, user-directed
2026-10-11) is the first consumer of that primitive, and a bucket-2 adapter
over it: every element on which a realm has filed the internal
`bobcat:exposure` record listener gets one observer of its own — root the viewport, thresholds `[exposure-area / 100]`, and a root
margin and a target margin folded from the element's margins by one rule:
`exposure-ui-margin-*`, always (neither native's per-element
`enable-exposure-ui-margin` nor its page config `enableExposureUIMargin` is
read), on the target margin of the same side, either sign, a `%` of the
element's own size; a positive `exposure-screen-margin-*` on the target margin
of the opposite side (native's `frameOfUIInWindow:` without the switch); a
negative one on the root margin of the same side (`borderOfExposureScreen:`).
A positive margin so grows the element's own rect, which the primitive clips,
and reaches an element a scrollport or a clipping page cut, where a root margin
cannot. Registration is the listener table's answer: `bobcat:element`
(`syncExposureListener`) files that listener while the element's
`exposure-id` is non-empty or any registration of any form for `uiappear` or
`uidisappear` is filed on it, and keeps it while the element is exposed, so
the `disexposure` owed under an old id still reaches it; the host registers
the element while `has_listener(node, "bobcat:exposure")` holds, and it hears
its element events while it also has a `uiappear` or `uidisappear` row
(`exposure_rows`, `hears_exposure`). `Exposure::set_registered` is told on
each change of either answer, from `open_listener`/`close_listener`. No
attribute registers an element by itself: the `setAttribute`/`removeAttribute`
host members, which know the `exposure-` names because the attributes belong
to every tag and so to no component, only re-arm an element that is
registered. The observer's handler turns each entry into an `ExposureChange`
(exposed or not: intersecting with a ratio of at least the area), and the
intersection delivery entry (`notify_intersection_observers`) dispatches
each `uiappear`/`uidisappear` (non-bubbling, detail from the target's own
attributes), then, for each change that has an `exposure-id`, one
`bobcat:exposure` event at the element: non-bubbling, not composed, with the
extras `exposureID`, `exposureScene` and `appeared`. The records a
`uiappear`/`uidisappear` listener queued — a stop, an `exposure-id` write —
join that entry's own. The record dispatches
run without a checkpoint of their own (`Checkpoint::Deferred`), and the
entry then checkpoints each realm it called once, so the record listener
(`exposureRecordListener`) queues web-core's record for each element and one
promise job (`flushExposureRecords`) sends one `exposure` and then one
`disexposure` list per entry through `bobcat:runtime`'s
`__BobcatSendExposureRecords` to that realm's BTS `GlobalEventEmitter`,
web-core's record shape and order. The realm holding the element's handle is
the one with the row, so a frame card's records reach the card realm and its
own BTS, and a freed element is sent no record. **The switch is per card,
over the realm's own element list** (user rulings 2026-10-16, native's per
card switch, and 2026-10-20: which elements are a card's is its realm's
knowledge, so Rust keeps no scope): `switchExposure(on, sendEvent, ids)`
takes the comma-joined node ids of the elements the calling realm
registered (`childElementIds`'s list encoding, `""` for none), and
`Exposure` keeps one stopped mark per registration. `bobcat:element` keeps
the realm's registered set (the elements it filed the record listener on,
in filing order, an id leaving with its listener or its collected handle)
and whether the realm is stopped; `__BobcatSwitchExposure(on, sendEvent)`
records that and hands the host the set, and `lynx.stopExposure({sendEvent})`
/ `resumeExposure()` on either thread are `switchExposure(0, sendEvent, ids)`
/ `switchExposure(1, 0, ids)` through it, so the page's stop leaves every
card detected and a card's stop leaves the page and every other card, its
own cards included. An element the realm registers while stopped is
stopped right after its listener is filed (`switchExposure(0, 0, id)`), so
it is not detected until the resume. A stop marks each listed registration
stopped — its observer stays armed and `Exposure`'s handler ignores what it
reports — and with `sendEvent` queues one `disexposure` record per listed
exposed element and forgets it as exposed, whatever an earlier stop did
(native sends its exposed set on every stop); a resume clears the mark of
each listed stopped registration and re-arms it from its attributes, so
what is visible then and not recorded as exposed appears again, and leaves
a listed running one alone. An id that names no registered element — never
registered, unregistered, freed — is skipped. The member answers the number
of records it queued — those with an `exposure-id`, `0` without `sendEvent`,
for a resume, or with nothing exposed — which is what a card let go of reads
to decide whether its last list owes a delivery entry (the `frame`
paragraphs in `crates/bobcat-core/docs/frame.md`). An element freed while stopped is let go of at its free,
sending nothing, as one freed while running is. Four user rulings (2026-10-11): the
geometry is the primitive's (viewport root, clipped through every clipping
ancestor, the ratio over the finally visible area), ui margins are gated by
the switch as documented and as native does, `lynx.stopExposure` stops
detection and `resumeExposure` re-observes (native's semantics, with
`sendEvent` defaulting to true), and delivery is one batch per update with no
timer. The defaults where the references differ are in
`docs/tracking/deviations.md`.

**Lynx layout events** (`crates/bobcat-core/src/main/layout_change.rs`,
user-directed 2026-10-20, `docs/layout-observer-design.md` §5) are the
second bucket-2 adapter, over two `terminus` primitives. `layoutchange` is one
document-wide `ResizeObserver` (created on the first registration) of which
every element with a `layoutchange` row in the listener table, any realm and
any form, is a `border-box` target — an `overlay`'s or `x-overlay-ng`'s
shadow `#dialog` in place of the box-less host, web-core's
`layoutChangeTarget` (`tree::layout_change_target`; `LayoutChangeObserver`
keeps the dialog-to-host pairs and dispatches at the host with the dialog's
rect). On each change of `has_listener(node, name)` for `"layoutchange"` or
`"layout"`, `open_listener`/`close_listener` add or remove the node's
`LayoutEventRegistration` (`layout_registration_follows`), as they tell
exposure: `ResizeObservation` is `LayoutChangeObserver::observe`/`unobserve`
of a live element, `TextLayoutReport` is
`Document::set_text_layout_reporting` of a `text` element. Its handler, bound to no node, only records the targets; the
epilogue runs `update_resize_observations` after its commit and posts the
resize delivery entry (`Page::post_resize_notifications`, before the
intersection one), which runs `terminus`'s broadcast and then dispatches
`layoutchange` at each recorded target, not bubbling, not composed, with the
extras `left`, `top`, `width`, `height` of `bounding_client_rect` (all `0`
for a target with no box); `bobcat:element` (`layoutDetailOf`) builds
`{id, dataset, width, height, left, top, right, bottom}`. Size changes only,
no transforms, no host origin subtracted, `dataset` included (rulings 1, 3, 4
and 8). A freed target leaves through the primitive; a target whose last row
left with a forgotten realm is unobserved by its next delivery. A `text`
element's `layout` row turns its text layout report on (a `layout` row on
any other element is ignored); after every commit the
epilogue's `queue_text_layout_events` takes `take_text_layout_reports`, reads
`text_lines` and the content box from `rounded_layout`, and queues one
`ComponentEvent::TextLayout`, which the component-events entry dispatches at
the `text`, not bubbling, not composed, with `lineCount`, `lines` (JSON text of
a flat `start, end, ellipsisCount` array) and `width`/`height`; the realm
builds `{lineCount, lines, size}`. A listener filed after the paragraph laid
out hears nothing until the paragraph's next committing re-layout (iOS's
per-renderer latch).
