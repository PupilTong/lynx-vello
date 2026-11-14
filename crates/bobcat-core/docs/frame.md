# bobcat-core: The frame element

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

**A `frame` is a custom-element ability** (user ruling 2026-10-14): what is
frame-specific lives in the `tree::frame` component and in the JavaScript
modules `bobcat:frame`/`bobcat:frame-boot`, and the host module keeps only
generic members. One PAPI over one document (user ruling: no second engine,
no scoped copy of the PAPI, no admission check or per-realm node set — the
host card and its frames are one trust domain). An MTS realm's
`ShadowRealm` child installs exactly the page realm's document, attribute,
readback, exposure, event-name, stylesheet and nested-page members over the
same `Rc<RefCell<DocumentSlot>>`, `ViewOutbox` and thread, through the same
`DocumentHost::install_members(engine, startup, retired)` (`retired` is
`None` for the page realm and the child's own flag from the
`ChildHostModules` seam); its startup strings answer `undefined`, it has
`Worker` members over a worker owner of its own, and it has no
`bobcat-internal:native-modules`. `createPage` and
`flushElementTree` are the same code in every realm (a child's flush may
park for the listed sheets and the painter binding as the page's would).
Node ids are document-global and each realm's `bobcat:element` has its own
handle table; a child's `__CreatePage` answers the page root
`__BobcatUsePageRoot(nodeId)` chose, a handle held by the module and never
registered for collection, like the page realm's. The four generic members:
`framePage(frame)` reads the nested page of a live `frame`
(`tree::frame::nested_page`), refusing a frame more than `MAX_FRAME_DEPTH`
(8) frames deep over shadow-including ancestors, so a card that embeds itself
stops; it keeps no state. `resetFrame(frame)` is `tree::frame::reset_frame`:
it `drop_element`s the nested page (inside a shadow tree, so `terminus` frees the
whole subtree, and each freed node's listener rows and intersection
registrations leave through the free path), clears the frame's shadow-root
sheets (`Document::clear_shadow_stylesheets`), and appends a fresh
`page[part=page]`, built as `constructed` builds the first: the nested page
belongs to the card, not to the frame (user ruling 2026-10-19), so the next
card's `framePage` answers a different node and the old card's realm never
meets the next card's tree. `adoptStyleSheet(url, shadowRoot?)` mounts on the
named shadow root (`style::add_*_to_shadow`, no listed sheet settled) or, with
no second argument, on the document after the listed sheets.
`closeRealm()` sets the calling child's `retired` (the page realm's throws
`the page realm cannot close itself`), so the release step frees it whoever
refers to it; after it, the child's `dropElement` answers `undefined` without
looking, because its handles' finalizers may still run before the realm goes
and name nodes `resetFrame` already freed. `dropElement` of a frame host does
nothing frame-specific: its shadow tree goes with it. Nodes a closed child
created and never attached stay until the view ends. A child's event
registrations are filed in `terminus`'s listener table under its
`RealmId::Child(key).listener_key()` (never `ListenerKey(0)`,
`RealmId::Page`'s); the
host keeps no record of which page a child renders, and a dispatch calls it
like any other realm (below); `closeRealm` — and the drop of its host state
when the release step frees it — removes its rows (`remove_listeners_of`).
The published listener names are the table's, over every realm: a
child closing `tap` leaves the page's open, and a name only a closed child
held closes when it closes.

**The frame's lifecycle.** The component builds the first card's nested page
(`page[part=page]`, web-core's `[part="page"]`) in `constructed`, and
`resetFrame` replaces it for each later card, and its `connected_callback`/`disconnected_callback` queue
`ComponentEvent::Frame { node, connected }` on the document's
`ComponentEvents` (a reaction queues, never acts, because it holds the
document). `MainThreadRuntime::dispatch_component_events` dispatches each
at the frame as an internal event, `bobcat:frameconnected` or
`bobcat:framedisconnected` — not bubbling, not composed, no payload, a
`ListenerFailed` like any listener's — and after a disconnection runs
`children.release_unneeded()` outside the call, so the retired child goes in
that entry. `__CreateFrame` and `__CreateElement("frame")` mark the handle
and file `bobcat:element`'s own listener for both names on it, which no app
can name, list or remove, so the listener table invites the realm that
holds the frame's handle — a frame a card created is that card's to load
and let go of, at any depth, a detached frame included, whose path starts
at itself — and a frame freed since (its page reset, its handle collected)
hears nothing. What happens is JavaScript's
(`packages/bobcat-element/src/frame.ts`, `frameConnectionChanged`):
`__SetAttribute(frame, "src", value)` hands the new value (`null` for a removal) to
`frameSourceChanged` after the native write, as it hands a `data` or
`global-props` write to `frameDataChanged` (both are `bobcat:element`'s
`STRUCTURED_PROPERTIES` of a `frame`, so an object is written as its JSON
text). The per-frame state — generation, `src`, whether a load is started,
the realm, the child's `dispose`, `updateData` and `updateGlobalProps`, and
the frame's `data` and `global-props` as JSON text, and how many cards it let
go of still owe their last list — lives on the handle, which every input
arrives with, so a frame whose handle another realm holds is nothing to this
realm. A `null` or `""` does nothing — the frame
keeps its card, as native's `FrameElement::OnSetSrc` ignores it and web-core's
`LynxView` keeps its running card when `url` is removed — and neither does a
value equal to the frame's current one, which both skip. Any other tears the
current card down (`teardown`, below) and loads the new one, connected or
not: a load bumps
the generation, resolves `src` once with `resolveUrl` into `U` (against
`ViewSources::base_url`, as the fetcher would) and calls `fetchResource(U)` —
`bobcat-source`'s `BundleInstaller` registers the card through
`ContainerSources`, its `root` at `<U>/root.js`, its StyleInfo at
`<U>/style-info.css` and its BTS boot at `U#background-thread`, before the
fetch settles — and when it settles (a probe hit's `true` in a microtask, a `Future` id through
its `then`), once no card the frame let go of still owes its last list,
creates a `ShadowRealm`, imports `bobcat:frame-boot`'s `boot`, `dispose`,
`updateData` and `updateGlobalProps`, and calls `boot(frameId, U, src, data,
globalProps, JSON.stringify(SystemInfo), __BobcatNativeModuleTable())` with
`src` as written and the frame's `data` and `global-props` as they stand
then. The frame keeps
`src` as written, which a later `src` is compared against and a failure
names. A `data` or `global-props` write keeps
text that parses to a plain object (a non-null, non-array object) and
ignores anything else; a removal makes what the next boot reads `{}`. Every
boot reads the last values written — the first, a new `src`'s, a
reconnected frame's — as web-core's `LynxView` renders every card with its
current `initData`/`globalProps` (native boots a new `src` with none; user
ruling 2026-10-15). Once a card booted, each write is also handed to it, no
deduplication: `updateData(json)` is `__BobcatUpdateData(json, "", false)`
(the card's `processData` with the default processor, then its
`updatePage`), `updateGlobalProps(json)` is `__BobcatUpdateGlobalProps`, the
page's own update paths, both of which post to the card's BTS as well
(`updateCardData`, `updateGlobalProps`); a write while the fetch or the import is in flight
reaches `boot` alone. A disconnection tears the card down, as
web-core's `LynxView` disposes on `disconnectedCallback`; a connection loads
the frame's `src` again when nothing is loaded or loading, as
`connectedCallback` renders (so a move re-renders). A frame handle collected
while it still has a card (given a `src`, never connected) disposes of it
through the module's `FinalizationRegistry`, so no orphan child stays alive
on its timers. Every step checks the generation, so a fetch or an import for
an earlier load starts nothing; a failure here (the fetch, the realm, the
import) is one `ScriptReported` naming `src`. A wrapped function is called
with no receiver: the `ShadowRealm` call wraps `this`, and an object `this`
is a `TypeError`. The child's boot (`frame-boot.ts`) imports
`bobcat:runtime` before `bobcat:element` and is the page boot, in the page
boot's order, minus the `Document` and the entry import: `framePage(frameId)`
and `__BobcatUsePageRoot(page)`, `__BobcatInitializeMTS` with the parent's
page data, `SystemInfo` and native module table, `__BobcatInitEntry(U)` (so
`__Card__` is the card's absolute URL, ReactLynx's `bundleName`, and
`chunkURL`/`styleSheetURL` name what the installer registered),
`adoptStyleSheet(bundleStyleSheetURL(U), parentNode(page))` — the frame's
shadow root — when `hasStyleSheet` holds it,
`loadModuleSync(sectionURL("root", U))` in a `try` of its own (a throwing
root is reported and boot still renders), then the frame's `load` — a
bubbling, composed `__CreateEvent(8, "load", …, {statusCode: 0,
statusMessage: "success", url: src})` dispatched at the nested page, whose
path crosses the frame's shadow root, so the realm holding the frame's
handle hears it retargeted to the frame (`bindload`, an ancestor's listener,
a `global-bindEvent`) before the card's own listeners do, both called
inside the host's `dispatchEvent` in the routed realm order (user rulings
2026-10-21 and 2026-10-23; a listener's throw is its realm's failure, which
the host reports, and boot goes on, unless a listener of the parent's let
go of the card during it, a new `src` written in `bindload`, which stops
boot there and answers `false`) —,
`__BobcatProcessInitData`,
`__BobcatConnectBackground(new Worker("bobcat:bts", {name: "lynx-bg"}),
data, backgroundThreadURL(U))` — the card's own BTS, which the child creates
and owns, so its messages are delivered to the child —, `__BobcatRenderPage`,
and a flush queued one microtask later, as the page boot queues its own. It
answers `false` for a failure it already reported. **Letting go of a card
sends its last `disexposure` list first** (user ruling 2026-10-20, option
A). `teardown` calls the child's `dispose(done)` with a `done` of the
frame's own (a wrapped function crosses the boundary), and `done` runs
`resetFrame(frameId)`, which frees the old card's elements with their
stopped marks, so the next card starts with nothing stopped, then whatever
waited for the reset. The child's `dispose` stops its own exposure with
`sendEvent` — `bobcat:element`'s `__BobcatSwitchExposure(false, true)`,
`switchExposure(0, 1, <the ids of every element this realm registered>)`,
which also stops each element it registers from then on — which answers
how many records it queued, and lets go of every card its own frames
loaded (`__BobcatCloseFrames(onClosed)`, which calls back once each of them,
and each card a frame of this realm let go of earlier whose list is still
owed, is done). When the stop answered 0 and no such card waits, all of it
happens inside `dispose`, as before: `__BobcatDispose()` —
`__DestroyLifetime` and the BTS's `dispose`, whose app cleanup (ReactLynx's
`destroyCard`, unmount effects) runs before the BTS acknowledges and the
Worker is terminated — then `done`, so the parent's `resetFrame` runs at
once; `closeRealm()` follows once the BTS answered. Otherwise both wait one
main-thread entry: the stop's records are dispatched at the card's elements,
still live because nothing was reset, by the host's next intersection
delivery entry, and `bobcat:element`'s `__BobcatAfterExposureRecords` runs
the rest once the first flush scheduled after the stop has posted the
card's `disexposure` list to its BTS — before `__BobcatDispose`, after which
`sendToBackground` posts nothing. Until then the card holds every handle of
its own with the record listener filed, so no collection frees an element
whose record is owed, and `done` holds the frame's handle, so the frame and
its nested page stay; the card goes on hearing what is dispatched to it
meanwhile. A load of the frame waits for the reset, so a new `src` — fetched
already, starting in a microtask — boots into the fresh page, never the old
one; teardowns that pile up while one waits reset once. The old child is
kept meanwhile because its worker runs; a BTS that never answers keeps it
until the view ends. Once a realm's disposal has started it handles nothing
its BTS says but the acknowledgement, as it already sends nothing, so a late
patch never reaches nodes `resetFrame` freed; its page root names a freed
node from then on, which nothing on its way to `closeRealm` reads. Its own
frames' cards close through `__BobcatCloseFrames` before the free, and their
trees go with its page. The page's own disposal module
(`MainThreadRuntime::begin_dispose`) closes the page's frames the same way,
with no callback, before `__BobcatDispose`, so every frame card with nothing
exposed has its BTS `dispose` queued on the shared worker thread ahead of
the page BTS's; their acknowledgements are not awaited, and a card that owes
a last list waits for a delivery entry the ending view may never run, so its
BTS `dispose` may never be posted. (`bobcat:runtime` cannot import
`bobcat:frame` to do this itself: the cycle that closes makes QuickJS leak
an object when a worker's `import("bobcat:element")` fails to link, which
aborts the runtime's release.) No
`load` event and no frame methods beyond the shared base set (native's
surface; user ruling 2026-10-15). **Events reach the card in its own realm**
(user rulings 2026-10-16, web-core's catch and keyboard; 2026-10-18, scheme
D; every realm at the path's top, 2026-10-19): nothing forwards between
realms, and a frame is a plain shadow tree to the dispatch. The kernel's walk
calls every realm the listener table invites, each once, at the path's top,
the page realm first (see the event paragraphs in
`crates/bobcat-core/docs/events.md`), and each walks the
steps it holds handles for, so the page's handlers on the frame hear a tap
inside the card first, with the frame as the target, retargeted, and the
card's own handlers after, at the card's node. A catch or a stop ends one
realm's walk alone, as each web-core `lynx-view` walks its own shadow root
(native's single propagation across both pages is not adopted), and a
listener that throws in either realm is that dispatch's `ListenerFailed`
after the other realm still heard the event. The card's `detail.x/y` and
pointer, wheel and touch `x/y` are relative to the frame's border box (its
`boundingClientRect`, read once per position dispatch, so no transforms; a
frame with no box answers the viewport's origin), while `clientX/Y` and
`pageX/Y` stay the viewport's. A card's `global-bind*` is reached by the
events whose path passes through its frame's shadow root, its tree's root,
as web-core's card listens on its own shadow root; a `keydown`/`keyup` is
web-core's document-level exception (`DOCUMENT_LEVEL_EVENTS`), so each card's
`global-bindkey*` hears every key, as web-core's nested bindings listen on
`document`, holding no step of the key's path and running its global pass
alone, aimed at each registering element. Non-composed events at a
card's node (`scroll`, image `load`/`error`, `uiappear`) end at the frame's
shadow root and reach only the card. The realms a dispatch calls are fixed
before its walk, so a frame an earlier listener removed still has its card
called in that dispatch; its `closeRealm` follows the disconnection in a
later entry. A frame an earlier listener gave a new `src` whose card has
nothing exposed frees the old card's page inside that listener, and with it
every registration the card held, so the card is skipped; one whose card
owes a last list keeps that page until the list is sent, so the card is
still called (`docs/tracking/deviations.md`, `<frame>`). The card's BTS is a BTS like the page's, on the group's
worker runtime, with the parent's `SystemInfo`; it shares the group's
`sharedData` with every other BTS and has the view's `NativeModules` (both
native's, where web-core gives a frame its own `sharedData` and none; user
rulings 2026-10-15). So a compiled ReactLynx card hydrates, runs its effects
and re-renders on a later `data`/`global-props` write, which its MTS
`updatePage` leaves to the BTS — web-core-e2e `api-frame-data-update`, ported
in `crates/bobcat-source/tests/frame.rs` (`docs/tracking/deviations.md`).
