# bobcat-core: Public surface and the two engine threads

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The public runtime is the opaque `LynxGroup`, `LynxView<F>` and `Painter`
facades plus the protocol-only, host-injected `ResourceFetcher`, draw-target,
OS-input and lifecycle-wakeup capabilities. The script engine is deliberately
not one of them: core owns its `QuickJS` realm, and an embedder sees only the
sanitized `script::ScriptError`. A view is built from one `ViewSources` —
the required `base_url` its entry and BTS entry resolve against by URL rules
(inside `create_lynx_view`, before any request; one that does not resolve is
the construction error `EngineError::InvalidUrl`, and an embedder gives its
fetcher the same base), `PageConfig`, owned font containers, an optional
default font family, author stylesheet URLs, the entry MTS module URL
(relative allowed), optional
`init_data` and `global_props` JSON text only the realm parses, and the
required `screen` metrics `SystemInfo` reports
and the required BCP-47 `locale` of its text
(`ViewSources::new(base_url, entry, screen, locale)`; a host with no screen — a
headless or offscreen capture — names `ScreenMetrics::for_viewport` of its
capture size explicitly) — plus a builder
turning the view's `ImageReports` into its `ResourceFetcher`; both go to
`LynxGroup::create_lynx_view` with device metrics.

**A view is built from a group, never on its own**: `LynxGroup::new` takes the
lifecycle wakeup and `StyleThreads`, starts `bobcat-workers` then `bobcat-main`
(handed one sender on it), and awaits the Stylo pool every view in that group
shares. Each thread builds its QuickJS runtime with `esm.rs`'s `build_runtime`
and keeps an `Err` instead of failing the group: every view then fails its
startup with it (`StartupFailed(LynxViewError::Script(..))`), and every
`Worker` on a failed worker runtime ends with `Failed`.

**Both engine threads are a `jobs.rs` `JsThread`: a tokio `current_thread`
runtime with a `LocalSet`, plus a FIFO of jobs its top loop runs between two
turns of that scheduler.** Tasks wait and route — a channel read, a deadline, a
lifecycle signal, a spawn — and touch no realm, no document and not the shared
`ScriptRuntime`; what a task does with what it read is queue a job and await it.
Jobs are the only place JavaScript runs, and they run outside every `block_on`,
which is what lets one park: `JsThread::wait` is a fresh `block_on` of the same
`LocalSet`, so during a synchronous stylesheet adoption or a `require` every
task on the thread goes on running while no other job does. Queued jobs run in FIFO order once the
waiting one returns, so an entry may hold the shared runtime and its realm
across its own wait. Every job of a page or a worker is queued with the
`JobKind` it was queued for, which a long task reports.

Each asynchronous wait is a task of its own. A view's tasks are its owner
(`serve_view`, whose one wait is the view's end), one task for the entry,
entering the realm when its answer arrives (the listed author stylesheets have
none: boot's first `__FlushElementTree` waits for them), one ordered consumer
of the command channel, one of the painter's metrics, one ordered consumer of
its workers' events, one future per resource load an import produced, per
`Future` a `.then` asked to settle and per `@font-face` rule, and one clock task
(`lifetime.rs`'s `serve_clock`) waiting on its realm's next timer deadline and
on the runtime-wide checkpoint generation; a `Worker` realm on `bobcat-workers`
has the same shape minus the document. Nothing is spawned per input: one
consumer reads each ordered stream with `while let Some(x) = rx.recv().await`.
Every task reaches the realm through one boundary, the realm driver's `enter`
(`realm/owner.rs`), which a view's page and every worker share: it queues a job
that runs the owner's `before_operation` hook, where a page syncs the
document's animation clock to the painter's, then one synchronous operation
under the borrows of the shared runtime and the realm, and then that
operation's epilogue. The epilogue is one function;
what only one role has is a hook of that owner's `RealmOwner` impl at a fixed
step. For a page the order is: the timers that came due, the commit — then the
`<text>` `layout` reports that commit left (queued as component events), the
resize observations that commit moved, the intersection observations that
commit or an adopted scroll moved, and the resize, intersection,
content-visibility and component-event deliveries it posts as entries of
their own (the resize delivery dispatches each resized element's
`layoutchange`; the intersection delivery also drains the Lynx exposure
changes — each `uiappear`/`uidisappear`, then each `bobcat:exposure` record
event, out of which the realm's Lynx layer sends the `exposure` and
`disexposure` lists — and is posted for them even when no observer crossed a
threshold, as after `lynx.stopExposure`) — the boot report once, the frame-post acknowledgement, the module requests
the operation left, the future settles, what the realm's `ShadowRealm`
children left unhandled, the `@font-face` loads, the release of every child
nothing needs any more, the next timer deadline, and the checkpoint
generation as of this entry. A child's timers run with the realm's own, and
its module requests and future settles follow the realm's. `Settles::settle` is
the epilogue alone, for a wake carrying no operation. What a failure is
reported as, and whether it ends the view or the worker, is the realm kind's
table in `realm/policy.rs`, read by the scene the failure happened in.
**Nothing of a view is served outside a job**: opening its realm is the view's
first job, queued before any of its tasks is spawned, so a burst that arrived
before the realm did is a job queued behind it and finds a document — and a
frame post is acknowledged by a job like everything else, so while any job
of the group is parked the acknowledgement waits with it. A `bobcat-workers`
consumer never awaits the deliveries it queued, because `Terminate` is in band
behind them: it queues one job per message and ends the worker the moment it
reads one, which is what releases a job parked on a synchronous wait and
discards the posts queued ahead of it.

A view owns its channels end to end, all `tokio::sync` and none addressed.
Three cross that link: a `ToMain` mpsc carrying commands in; a `ViewNotice`
mpsc carrying lifecycle events, resource asks and the focused editing host's
`TextInputState` back (the last as `ViewNotice::TextInputState`, sent from a
page's epilogue after its commit when it changed, which `pump` hands on as
`EngineEvent::TextInputStateChanged`); and one
`watch<Published>` carrying what an observer wants the *latest* of — the newest
committed frame, the listener-name set, and the newest serviced frame post.
Commands are a FIFO because their arrival order is what they mean; a frame is
not. `ToMain` carries a `PageUpdate` (the data, global-prop, global-event and
reload commands a host accepted after observing MTS boot, in host FIFO order),
a `DispatchEvent` (one event's type and target, plus the payload the router
decided as values: the position, the wheel delta, the timestamp and — for the
four touch events — its touch points), an `Edit` (one `terminus::link::editing::Command`
for the focused editing host, from `LynxView::edit`; main dispatches the
`beforeinput`/`input`/`composition*` events it owes around the change, which
no listener can stop), a `Vsync` (the display-frame reading a realm that called
`requestScriptFrame` asked for), `Posted` (payload-free: the view's
`ScrollMailbox` holds the painter's latest offset per scroll container and one
coalesced frame post — a timeline reading, the sequence number the
acknowledgement reports, and a fence: the count of commands sent before it,
which main applies first — and at most one marker is queued per take),
`ImageEvents`
(completed or failed host loads, and documents handed over as their bytes —
no variant can carry pixels, which makes "`ImageData` never crosses a
channel" a property of the type), and a
`ModuleCallback` (a native module's answer to one function argument of a call
the MTS realm made, or its release). The fence counts every command but that
last one. The seat's `CommandSender` counts what it sends, on the embedder's
thread; a `ModuleCallback` is sent from whichever thread drops the module's
callback, through the weak sender `CommandSender::uncounted` hands out, so
neither the seat's count of commands sent (`CommandSender::sent`, which the
painter reads at a frame post) nor the page's count of commands applied
(`Page::apply`) includes it, and no frame post waits for one. The
painter's device metrics are **not** a command: they ride a
`watch<Option<Viewport>>` on the view's seat, `None` until a painter attaches,
because an unbound `__FlushElementTree` parks the job it runs in on that very
watch and no other job would run to read a command.

`create_lynx_view` sends the far half of that link to the group's thread and
builds the fetcher in place on the calling thread. It is **synchronous and
takes no draw target**. Its `width`, `height` and `device_pixel_ratio` are the
create-time viewport the document is built at and works at until a painter
binds; they are not validated, because no target is built from them, and an
attached painter's metrics supersede them. The view's `F` parameter is that
view-owned fetcher; the wakeup is a separate group constructor generic held by
`bobcat-main`. Construction returns a loading view at once, whose boot outcome
arrives through `pump`; constructor errors cover attachment, an unknown default
font family and two native modules claiming one name.

**That same call hands the fetcher the view's startup sources.** It validates
the fonts and default family first — a `terminus::link::TextContext`'s business, with no
document and zero fetches on failure, which is why the check has to be here
rather than a turn later — and then requests each stylesheet in the order the
view listed them, followed by the entry module, on the embedder's thread,
before it returns. Only the built text context and the answering one-shots
cross to `bobcat-main`. The entry's is read by a task of the view's owner as it
arrives, which completes the module boot imports the entry by its URL; an
entry the fetcher could not load fails the startup with the fetcher's own
error, before any of it runs. The
sheets' go to the realm's `DocumentSlot`, and **boot's first
`__FlushElementTree` waits for every listed sheet, success or failure, before
the document enters the style pipeline**, mounting them in listed order — so
the cascade order between several listed sheets is the listed order, and no
frame is published without them. A sheet that failed to load, or that the
fetcher answered with something else, makes that flush throw
`loading stylesheet <url>: <reason>`, which fails the boot with
`StartupFailed(LynxViewError::Script(..))` naming the sheet; the entry that
resumed boot has reported the same rejection just before, as a
`ScriptRunError` with the same message. App code can settle the listed sheets
first — an `adoptStyleSheet` or a `__FlushElementTree` inside the entry's
evaluation — and catch the failure, so `DocumentSlot` keeps the first one and
every later settle throws it again: boot's own flush still fails the boot with
it, and the epilogue's implicit commit stays off. The realm itself opens
immediately, before any answer has arrived, so the fetcher's IO,
the whole of boot and the first frame's encode all overlap the painter the
embedder builds next, and `LynxView::pump` services every *later* request — imports,
`adoptStyleSheet`, worker scripts, fonts, plain fetches — and the view's images
in ordinary turns. The default family is prepended to the `system-ui`,
`sans-serif` and `serif` generic maps, so a Wasm embedder can supply its
otherwise-absent system-font backend without baking a font into core.
`ViewSources::fallback_fonts` does the same for script fallback: every family
in those containers is appended to every script's fallback list
(`TextContext::register_fallback_fonts`), so it shapes the clusters no family
of the run's own `font-family` covers — the Pages site covers Chinese that way
with a system font read through the Local Font Access API. A script given a
list stops asking the platform for its own, so an embedder with system fonts
leaves the field empty. A default family name neither the containers nor the
platform has fails the construction with `EngineError::UnknownFontFamily`.
The platform's own fonts are discovered once per process, by the first text
context built — inside `create_lynx_view`, which builds every view's — and
`bobcat_core::preload_system_fonts()` lets an
embedder run that discovery early, on a thread of its own: process-wide and
idempotent, and a view built before it finishes waits for it rather than
discovering again. Both native embedders in this workspace call it at startup,
`bobcat` before it decodes its input and `bobcat-server` before it binds.
The platform's per-script fallback is asked only for a character the run's
own families lack: `TextContext::set_locale` files every script's fallback
list empty, so fontique, which looks a segment's list up at every segment
start, asks the platform for nothing; before each shaping seldon tests the
text against the faces its spans' own `font-family` lists match
(`TextContext::cover`), and fills the list of each script a missing
character is written in (its Script_Extensions), plus the list fontique adds
to every query (`(Hani, None)`, fontique's own constant, the one a
Common-script character such as `★` is found in), from one table for the
process. That table asks the platform the first time any context in the
process needs a (script, language) and answers every later view from there;
the answer goes under the script's language-named key and its language-less
one, for every script alike. On a Mac that first ask is up to a few
milliseconds, about 8 ms for the first language-less one in the process,
which has CoreText build its whole default cascade list. A page whose text
its fonts cover asks nothing. `LynxGroup::preload_text(locale, scripts)` —
synchronous on the calling thread, for a host whose first view comes later
than its group or only on some condition — runs the discovery and pays those
first asks for the scripts the host names (ISO 15924: `Hani`, `Arab`,
`Thai`, …). A context with fallback containers of its own, or without system
fonts, files nothing empty and asks nothing.

`ViewSources::locale` is the language of the view's text as a BCP-47 tag,
Lynx's `LynxEnv.locale`: the host sets it for the process, from the OS's
preferred language unless it names its own, and Lynx iOS reads it to choose
the CJK cascade family (`LynxFontFaceManager.m:440-462`). Here it becomes the
locale of every parley run, which fontique's fallback key carries to the
platform, so a Han cluster no family of the run's own `font-family` covers is
set in PingFang TC for `zh-Hant`, PingFang HK for `zh-HK`, Hiragino Sans for
`ja` and Apple SD Gothic Neo for `ko` where the platform's default is PingFang
SC. Each script's answer for the language is also filed under the script's
language-less key, for every script alike; that is how the language-less Han
key fontique appends to every query (its workaround for Common-script
punctuation in a run with no Han character) sets that punctuation in the
language's face once such a character is missing from the run's fonts.
Every embedder names one: `ViewSources::new` takes it.
Lynx has no `lang` attribute and no `SystemInfo` language field, so nothing
per element overrides it and script is not told it. A tag that does not parse
fails the construction with `EngineError::InvalidLocale`. `bobcat` names it
from `--locale`, else from the POSIX locale environment, else `en-US`, and
preloads the
scripts `--preload-scripts` names (comma-separated ISO 15924 codes, none by
default) through its group before building the view; `bobcat-server` names
`en-US`; the Wasm facade names `en-US` until `setLocale(tag)` names another.

Dropping a loading view marks source work cancelled and stops that view before
QuickJS begins. One `tokio_util::sync::CancellationToken` per view is minted on
the embedder's thread and cancelled there by the view's drop, by a fatal
lifecycle event, and by a guard on every exit from the view's owner, so a host
holding a `SourceCompletion` reads cancellation without waiting for a turn. It
is the view's end signal: the owner reclaims its ordinary tasks, then waits for
MTS JavaScript disposal before releasing the realm. Workers have independent
tokens and stay live for that exchange. Commands queued behind that release are
discarded rather than applied — the one command consumer reads the token at
each wake, before applying anything — while a burst already inside an entry
finishes. The view's handle on the host resource system goes with the release,
so past one a painter adopts no commit whose pixels it is not already holding.
Fetchers skip cancelled queued work; IO or synchronous JavaScript already
executing may finish, late source results are discarded, and the group and
other views keep running.

**A view spans two threads**: the embedder's own — whichever created its
`LynxGroup` — which owns the window, the input capture, the surface (the one
call macOS allows nowhere else), the host's whole resource system and the
`Painter` (routing, gestures, scrolling, composition and every GPU call), and
the Lynx main thread (document + realm). Neither half can leave it: a `Painter`
is `!Send` because its target is, and a `LynxView` is `!Send` because it holds
`Rc`s of the group and of the fetcher — the only shape the browser allows,
where `wgpu`'s handles are not `Send` under shared memory and an
`OffscreenCanvas` cannot be transferred on again. Dropping a view cancels its
source work, detaches its image inbox, then closes its command channel, the
goodbye its task ends on; the group handle drops last, so the group's threads
are joined only once nothing is left on them.

**Views in a group share one thread, one `QuickJS` runtime and one Stylo
pool.** `create_lynx_view` is the only way to build a view, because naming the
group is the only way to say which thread it runs on. One group per thread and
one thread per group: a group hands out `Rc`s of what it owns, so it is `!Send`
and `!Sync`, and the thread that creates it is the thread every view in it
paints on, which is why one `EventRequester` serves the whole group. Views take
turns — every entry into a realm is one synchronous stretch — so a second view
costs no second heap, module graph or set of workers, at the price of never
restyling in parallel, on the assumption that a person drives one view at a
time. A host that needs two pages genuinely parallel gives them a group each,
on a thread each.

`bobcat-main` builds the group's one `terminus::link::StylePool` — sized by the
`StyleThreads` passed to `LynxGroup::new`, `Auto` being the usual choice —
before any view attaches, and every document it carries holds an `Rc` of it.
Stylo's bloom filter and style-sharing cache are per-OS-thread borrows held for
a whole traversal, so two documents traversing on one worker at once is an
aliasing bug. Two facts rule that out: different groups draw from disjoint
pools, and documents in one group cannot traverse at once.

**`bobcat-main` is index zero of its group's pool**, taken over in place by
rayon's `use_current_thread` — which is why the pool can only be built on
`bobcat-main`, and why `StyleThreads` counts it: `Fixed(3)` starts two threads,
not three. A lone view therefore restyles with the same parallelism and the
same inline root closure Stylo's global pool gave it. The takeover is
permanent: rayon leaks about 25 KB per pool (the managed threads still exit on
drop; the `WorkerThread` box and `Registry` do not) and refuses a second pool
on the same thread forever. **That refusal is why the pool has to be the
group's rather than any view's.** A host that replaces groups — every
`BobcatRenderer::load` — pays that 25 KB per replacement, in the same Wasm
linear memory. `StyleThreads::Sequential`, and `Auto` where the pool would have
held `bobcat-main` and nothing else, gives a group no pool at all and traverses
on `bobcat-main` alone, a configuration rather than a fallback.
`terminus::link::MAX_STYLE_THREADS` is six, counted the way Stylo counts its own six: a
ceiling, not a tuning knob, because Stylo indexes its per-traversal
thread-local storage by Rayon thread index into an array that long, so a wider
pool is a construction error rather than a silent clamp, reported by
`LynxGroup::new` like any other boot failure. **Wasm takes the same path.**
`navigator.hardwareConcurrency` reaches `StyleThreads::for_parallelism`, which
is `Auto`'s own arithmetic, so comparable hardware gets the same pool on both
targets and the facade does no thread arithmetic of its own.
