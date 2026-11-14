# bobcat-core: The Element PAPI runtime

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The members that answer with a list encode it in the return string, since
the boundary's value type carries no array: `attributeNames` and
`getComputedStyleMap` as the length-prefixed record `setInlineStyles` accepts,
the latter a flat name-then-value sequence; `childElementIds` and
`queryElementIds` as comma-joined ids — neither of which needs a length
prefix. `callElementMethod` answers a number when the method has no data:
the status code, `0` for a success, `3` for no such method, `4` for params
it refused, `7` for a state that refused it (a dialog's
`InvalidStateError`), `8` for an operation that failed; a method with data,
`boundingClientRect` included (`left`, `top`, `width`, `height`), answers a
length-prefixed record whose first field is the code. It matches no name
itself: every name is the element's component's
`terminus::link::CustomElement::invoke`, reached through
`Document::invoke_element_method`, and on its `NotFound` the base set every
element has (`tree::base_methods`: `boundingClientRect`, `scrollIntoView`,
`scrollTo`, `scrollBy`). The params cross as JSON text, and as the empty
string for `boundingClientRect`, which reads none.

Each call is a plain owner-thread mutation, and `__FlushElementTree` runs the
style + layout + paint commit and publishes one immutable `Arc<CommittedFrame>`
on the view's watch. `Node`'s arena backpointer is a raw pointer, so a
`Document` is not `Send`, and its realm never leaves `bobcat-main`. The
boundary validates primitive arguments, live IDs and tree-mutation
preconditions before entering `terminus`, returning misuse as a JavaScript exception
(unexpected internal panics remain fatal on abort-only Wasm). An unflushed
batch may present once its evaluation ends — web-core's visibility model.

Beside the host module, both runtimes register the same built-in ESM sources
in QuickJS's loader: `build_runtime` registers `BUILTIN_MODULES`, the one table
in `esm.rs` beside the specifiers, and the list with its TypeScript sources is
in `packages/bobcat-element/AGENTS.md`. A realm's host modules decide
which of them it can use. A module written for the other realm kind fails at
link with a `SyntaxError` naming a member its realm's `bobcat-internal:host`
lacks (a worker importing `bobcat:element`, `bobcat:runtime` or
`bobcat-internal`), or at load with a `ReferenceError` naming a host module
its realm does not declare (an MTS realm importing `bobcat:worker`,
`bobcat:bts-runtime` or `bobcat:bts`, each of which imports
`bobcat-internal:worker`).
`build_runtime` also reserves the prefixes `ENGINE_MODULE_PREFIXES`
(`bobcat:`, `bobcat-internal:`), so any other name under them — one no runtime
registered and no realm declared — fails its `import` or `require` in the
realm with a `ReferenceError` and is never sent to a fetcher.
`bobcat-internal`, which has no colon, is registered on both runtimes, so an
`import` of it always finds its source and never reaches a fetcher. The
reserved prefixes do not cover it, though: a `require` of it in a realm that
has not imported it still goes to the host's synchronous loader, which asks
the fetcher for it. `bobcat:bts` is the BTS Worker's URL, a registered module
like the rest; `bobcat:boot` is the MTS boot module's own specifier, evaluated
once per realm and never registered. A worker realm has no module of the
engine's own around its script: its root module is the module at its URL,
loaded by that URL, and for a URL the realm has no source for that load is a
module request the worker makes and completes as any import's, under the
request URL, as a module of its own realm, so it is never registered on the
runtime, keeps its own line
numbers, and runs with its response URL as `import.meta.url`. The Element module
imports native operations directly from `bobcat-internal:host`; no host object
and no element member is installed on `globalThis`.

The PAPI runtime exports the supported Element PAPI only as named ESM bindings,
which a card's entry receives through the import `bobcat-source` prepends to
its body, and **the header table of `packages/bobcat-element/src/element-papi.ts`
is the authoritative enumeration** — it names every member and what backs it, and
`MTS_CHUNK_PREAMBLE` in `esm.rs` imports exactly that set. By kind:
every ReactLynx Snapshot constructor, `__CreateFrame` included; all six tree
mutations; the properties and queries a Snapshot's `create`/`update` functions
write through and read back, among them `__SetInlineStyles` and the name-based
`__AddInlineStyle`, with `__SetCSSId` writing `l-css-id`; the readback pair
`__InvokeUIMethod`, whose UI methods are `boundingClientRect`,
`scrollIntoView` and CSSOM-View's `scrollTo`/`scrollBy` on every element, a pager's
`selectTab`, a swiper's `scrollTo`, a `scroll-view`'s
`scrollTo`/`scrollBy`/`getScrollInfo`, a `list`'s
`scrollToPosition`/`scrollBy`/`getScrollContainerInfo`/`getVisibleCells`, an
`x-refresh-view`'s `finishRefresh`/`finishLoadMore`/`autoStartRefresh`, a
scroll coordinator's `setFoldExpanded`, a `text`'s `getTextBoundingRect` and a
dialog's `show`/`showModal`/`close`/`requestClose`, and
`__GetComputedStyleByKey`, neither of which commits anything
— both read the last completed pass and leave the decision to flush to the
caller; the event
registration and propagation members, `__AddEvent` and `__AddEventListener`
included, with native's synthetic pair `__CreateEvent`/`__DispatchEvent`;
`__CreateList` with `__UpdateListCallbacks`; and
`__FlushElementTree`. Everything else is not implemented, `__DropElement`
(which no web-core generation has) included; a bundle reaching
for another member fails at the missing name with a precise `ReferenceError`,
not silently. `__CreateEvent` stamps its event with the host member
`timelineMilliseconds`, and `__DispatchEvent` calls the host member
`dispatchEvent`, which runs the routed dispatch's own body (`run_dispatch`)
over the page realm's `RealmCore`: every realm the listener table invites
is called at the path's top step in the kernel's order, the calling realm
included, which walks the very event object it left pending, the others a
structured clone of the event's `detail`, and then the components on the
steps below (`docs/tracking/dom-events.md`, "The synthetic dispatch entry").

`__SetInlineStyles` keeps the whole-value policy in JavaScript: a string is one
`style` attribute write, a record crosses in a single `setInlineStyles` call as
a length-prefixed payload — `<utf16Length>:<text>` fields, name then value, in
enumeration order — from which the host builds one declaration block from
empty, so a value may contain any character, `;` included, without escaping.
Ordinary camelCase keys are hyphenated; case-sensitive `--*` custom property
names pass through unchanged. The host operation implements the name/value
subset of CSSOM `style.setProperty` (no priority argument, so an embedded
`!important` is invalid) and intentionally has no numeric-style-id variant:
`__AddInlineStyle` updates one named property in the existing block and removes
it for empty/nullish values, and numeric Lynx CSS property IDs remain
unsupported on both surfaces. `__CreateList` consumes only its numeric
parent-component argument; the callbacks it and `__UpdateListCallbacks` file
are read by `__SetAttribute(list, "update-list-info", …)`, the one name that is
a list command rather than an attribute. That member is the list *data*
protocol: the batch is serviced in a microtask — the callbacks a card files
immediately after writing the operations are therefore the ones that serve it
— every `insertAction` position gets a placeholder `list-item` carrying the
entry's platform info (`item-key`, `estimated-main-axis-size-px`, `full-span`,
the sticky pair), and every `removeAction` child is removed, a built cell
handed to `enqueueComponent` first, the i-th removal taking the child at
`position - i`. A placeholder becomes its cell only when it stops skipping:
the list listens for `contentvisibilityautostatechange` while it holds
placeholders, and its listener calls `componentAtIndex` with the
placeholder's current index and swaps the cell in, then builds the run of
placeholders after and before it that the relevance region covers, counted at
a built sibling's size from the host's `scrollRelevanceSpan` (`fillAround`).
The first placeholder batch
writes `l-lazy-cells` onto the list, under which the UA sheet keeps the size
estimate for placeholders alone and makes them no scroll anchor (user ruling 2026-10-27,
native's lazy `componentAtIndex` rather than web-core's build-everything;
recorded in `docs/tracking/deviations.md`). A batch reads the list's children
once and keeps that copy in step with its own edits, so it is linear in the
list's length. `updateAction` is ignored and `componentAtIndexes` is filed and
never called, as in web-core. Cell *recycling* — a pool, a window,
`enableReuseNotification` — remains part of the unimplemented list surface.

`__SetCSSId` writes web-core's `l-css-id` attribute on each element (a
non-zero id sets it, 0 or a nullish id removes it), and `bobcat-source`'s
`lower_style` repeats every rule of a scoped `StyleInfo` fragment once per
importing fragment with `:where([l-css-id="N"])` at the end of the subject
compound, so stylo's own attribute matching does the scoping. A rule whose
importers include fragment 0 keeps only its unguarded selector; web-core's
extra guarded copies would match a subset of it. Nothing reads
`enableRemoveCSSScope`: a bundle built with it carries only fragment 0 and
never calls `__SetCSSId`. Deliberately absent: the `l-e-name` entry-name guard
(`entryName` is ignored; it separates a lazy bundle's own `StyleInfo`, which
is not mounted) and parent-component css-id inheritance (ReactLynx creates
every element under the page, whose css id is 0). `docs/style-assumptions.md`
§D.16 records the decision.

`Document::drop_element` frees exactly the node the collected handle named: its
**element** children are unlinked and go on as detached roots, each held by its
own handle, while what no handle could ever name goes with it — host-owned text
nodes and a host's shadow tree in full. Generated `raw-text` content has no DOM
node.
