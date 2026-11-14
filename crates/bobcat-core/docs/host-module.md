# bobcat-core: The host module, realm construction and ShadowRealm

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The private `MainThreadRuntime` registers the native QuickJS ESM
`bobcat-internal:host` as one Rust-backed named function export per member, and
`packages/bobcat-element/src/native.d.ts` is the authoritative enumeration: a
`declare module "bobcat-internal:host"` block for the MTS realm;
`"bobcat-internal:worker"` for a worker's, which carries `postWorkerMessage`,
`closeWorker` and the one-shot `workerName`, and nothing else, the BTS's
included; and `"bobcat-internal:native-modules"`, which every realm kind
declares, the MTS realm included, for `invokeNativeModule` alone. The MTS
members group as the document's own life, tree vocabulary over numeric
`NodeId`s, attributes and style, selector queries, the commit, the event
registrations (`listenerOpened`/`listenerClosed`), the synthetic dispatch
pair (`dispatchEvent`, `timelineMilliseconds`; `dispatchEvent` is the one
member registered re-entrant, because it calls realms, its own included), timers, the one-shot
startup strings the realm alone reads (the page-data triple handed over as
plain JSON and processor-name strings, and the native module table it posts
unread to its BTS), the stylesheet pair, the diagnostics pair, the
display-frame demand, the three worker operations, and the nested-page
members `framePage`, `resetFrame` and `closeRealm`
(`crates/bobcat-core/docs/frame.md`).
Those four one-shot strings do not arrive separately: they, the entry's
resolved URL, the BTS entry's, the listed stylesheets' answers and the screen
are one `RealmStartup`, which is everything a realm is opened with and nothing
that is ever updated — `LynxView::update_data`, `update_global_props` and
`reload` reach the realm through `ToMain::PageUpdate` and never touch it.

Every realm, MTS or worker, is opened by the one constructor
`realm::open_realm` (`crates/bobcat-core/src/realm/mod.rs`). It creates the
realm, enables module loading and installs the core every realm has under
`bobcat-internal:host`: `requestScriptFrame`, `setTimer`/`clearTimer`,
`waitFuture`/`takeFuture`/`settleFuture`, `fetchResource`, `resolveUrl` (a URL
resolved against the view's base, as the fetcher resolves what it is handed),
`resolveModuleUrl`/`loadModuleSync`, and the diagnostics pair
`reportScriptError`/`logScriptMessage`, which send `ScriptReported` and
`ConsoleMessage` through the realm's own `HostOutbox`, and then the global
`ShadowRealm` (below). Registering a host member takes no runtime borrow: it
enters no JavaScript, and a child is furnished from inside the parent's entry,
which holds the runtime. Every other host module
is a parameter of that call: `MainThreadRuntime::new` passes the document,
stylesheet, startup-string (the module table among them), event-name and
nested-page members, the `Worker` members and `bobcat-internal:native-modules`, and
a worker passes `bobcat-internal:worker` and
`bobcat-internal:native-modules`, the same `invokeNativeModule` in both. So
is what the realm's `ShadowRealm` children get beyond the core, a
`realm::child::ChildHostModules`: the MTS realm passes its document members
and the `Worker` members over a worker owner of each child's own (the
`DocumentHost` in `main/runtime/lib.rs`), a worker `None`. It is handed the
`ChildKey` the child will be kept under, allocated before it runs, and
answers the child's `ChildHostState`, which the child holds and drops after
its realm. The constructor has no role field; it is told
two things about a realm: the key its display-frame demand is reported under,
`None` for MTS and the worker's key for a worker, and the `ScriptSource` its
diagnostics carry, `Main` for MTS and, for a worker, `Worker { id, url }`
over its key and its URL, whatever that URL is. What it
answers with, `RealmCore { engine, children, timers, futures }`, is the first
field of both `MainThreadRuntime` and the worker thread's `WorkerRealm`; the
MTS realm holds it as an `Rc<RealmCore>`, the one strong handle, because its
synthetic dispatch member reaches the same table through a `Weak` from inside
a JavaScript call, so every call into a realm (`ScriptEngine`'s export calls,
module loads, checkpoints) takes `&self`.

The core ends with the TC39 **`ShadowRealm`** constructor, so it is a global
of every realm kind — the MTS realm, the BTS and every plain `Worker` — a
bucket-1 capability like the W3C `Worker` (user ruling, no fork change). The
bridge's hook (`Context::install_shadow_realm`) hands each `new ShadowRealm()`
to `realm/child.rs`, inside the construction: the child gets module loading
and the same core, installed by the same `install_realm_core`, over a timer
table and a future table of its own and the parent's `HostOutbox`, thread and
`ScriptSource`, and a `ShadowRealm` constructor of its own, and then its
owner's `ChildHostModules`, installed over the child's `retired` flag, which
`adopt` creates first and stores in the list with the child: an MTS realm's children,
grandchildren included, get the page realm's document members (the
`frame` paragraphs in `crates/bobcat-core/docs/frame.md`) and `Worker` members over a worker owner of their
own, so `bobcat:element` and `bobcat-internal` link in them, and a worker
realm's get nothing more, so `bobcat:timers` links in one and
`bobcat:element` does not. A worker realm's child has no worker member, and
no child has `bobcat-internal:native-modules`.
`requestScriptFrame` is a
no-op there: the frame demand is one flag per realm key, and a child under its
parent's key would cancel the parent's demand with its own `false`. The child
is then kept in `RealmCore::children`, a flat `ChildRealms` list that holds
grandchildren too and is declared after `engine`, so the parent realm and its
`ShadowRealm` objects are freed first and the children after; the hook holds
the list weakly, because a strong one would be a cycle through the child's own
hook. The child's `ScriptEngine` shares its parent's `HostPanic`. A child has
no entries of its own — its code runs inside the owner's — so the owner's one
epilogue services it: its due timers after the owner's own (through each
owner's `run_due_timers`, so a page settles a child callback's removals in
the same batch and a worker that called `close()` runs none), its module
requests (`load_child_module`) and future settles (`settle_child_future`)
after the owner's, each completed in the child, which it finds by key and a
released child drops; then one unhandled rejection per child and then the
owner's own, reported under `Scene::ShadowRealm`, because an entry's
checkpoint reports for the owner's realm alone and a child's operation can
leave a rejection in the parent (an `importValue` that rejects); and the
earliest child deadline joins the owner's. The epilogue's last step before the
deadline frees every child nothing needs any more — no `ShadowRealm`, wrapped
function or `importValue` of another realm refers to it
(`is_referenced_across_realms`), no timer is armed, no load or settle is in
flight and its `ChildHostState` is not busy — an MTS child with a worker of
its own still running is, because what that worker says is the child's, a
card's BTS acknowledging its disposal included; a retired child goes all the
same — outside every JavaScript call, because freeing a realm collects the
whole runtime; nothing else frees a child while its owner lives but the
release a frame's disconnection runs right after its disconnection event,
which is the same step, and a child the hook could not furnish is kept retired for that step. A child's
diagnostics carry the parent's `ScriptSource`. The list keeps each child as
an `Rc` and never takes one out for an operation: the operation clones the
child's handle under a short borrow and runs with the list free, so a
`new ShadowRealm()` in the child's code meets no borrow, and a call into a
child whose own operation is further up the stack (a synthetic dispatch from
its timer callback that invites it) still finds it; only the release step
takes a child out.
