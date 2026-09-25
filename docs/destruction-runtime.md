# BTS disposal and Worker ownership

MTS owns BTS cleanup as a JavaScript message exchange. Its disposal Promise
posts `dispose` to the existing Worker, waits for `disposed`, then calls
`Worker.terminate()`. Explicit engine `__DestroyLifetime` notifications share
that Promise. Reload does not dispose the application.

The BTS message handler calls the current `app.callDestroyLifetimeFun` with the
app receiver and no arguments. A throw is reported through `lynx.reportError`;
the handler still replies after an ordinary `await` boundary, like web-worker-rpc.
It does not await asynchronous work returned by the framework's synchronous
hook. Ordinary BTS messages, timers and imports continue until MTS terminates
it. There is no native app hook or special promise-job drain, and the exchange
does not read the BTS worker's `WorkerRole`: the role says which kind of worker
a `Start` is for, and disposal is the same message exchange either way.

Disposal remains deliverable during an unfinished BTS entry import, cleaning up
any hook already installed. Unlike web-core's readiness wait, this cannot wait
for the entire entry: a released view no longer services its resource fetcher.
An already closed or failed Worker also completes the MTS disposal wait: MTS
keeps its Worker reference, but disposing a view whose BTS already ended
completes without posting `dispose`, and a post to an ended Worker would in any
case be dropped by the host.

## The MTS realm boundary

Releasing `LynxView` cancels ordinary view work. The page owner then starts the
ordinary `bobcat:dispose` ESM, whose top-level await waits for the MTS JS disposal
Promise. It continues routing Worker events through the existing inbox until
that evaluation finishes. Only then is the MTS realm and document released.
Rust neither parses the disposal messages nor calls the BTS app hook.

Each Worker has its own cancellation token, including its entry/import source
completions. It is not a child of the view token. MTS event routing holds
`WeakRef<Worker>` values, and a JS `FinalizationRegistry` releases the native
sending handle when its Worker object becomes unreachable. Explicit termination
uses the same release path and unregisters the finalizer. Thus an unreachable
ordinary Worker can end while the view stays alive, and a reachable Worker does
not end just because the host cancelled the view.

At realm release, remaining senders close naturally. Host functions hold weak
references to the channel owner so finalizers queued during realm destruction
cannot prolong its lifetime. Rust has no `WorkerOwner::drop` termination loop.
This GC ownership policy is the user-selected Bobcat behavior; collection is
not prompt or a replacement for explicit termination when timing matters.

What a page has to know about it: a `Worker` reachable only through its own
`onmessage`/`onerror` handler is a cycle rather than a root, so it is
collectable, and a message the worker already posted is then dropped where the
routing weak reference is read — silently, because there is no object left to
dispatch at. Every view of a group shares one `QuickJS` runtime, so the
collection that takes a page's workers away can be the one a sibling view's
boot runs; nothing about it is the page's to schedule. A page that waits for an
answer keeps its worker named until it has one. Browsers diverge here: a
running worker keeps its `Worker` object alive (Blink's `HasPendingActivity`),
so the construct-handler-post shape delivers there regardless of references.
`main::runtime::worker_tests::a_message_for_a_collected_worker_handle_is_dropped`
and `a_named_worker_survives_a_collection_and_still_delivers` pin both sides.

## Object destruction observers

`lynx.getNativeApp().createJSObjectDestructionObserver(callback)` follows
web-core's implementation: return an ordinary `{}` and register it with a
`FinalizationRegistry` whose cleanup directly invokes `callback()`. The registry
holds the callback without directly retaining the observed object. The object
is extensible, and this wrapper adds no argument checks, HostObject proxy,
error reporter or app-destroyed filter. GC and the JS runtime own cleanup timing
and exception reporting.

ReactLynx uses this object on `MainThreadRef`; its callback sends the existing
`releaseWorkletRef` Context event. This layer adds no cross-thread node API.

## Validation

Real QuickJS tests cover retained and cyclic unreachable Workers, GC during a
pending source request, independent view/Worker cancellation, channel closure,
BTS disposal acknowledgement and repeated notifications, throwing hooks,
disposal during entry imports, and already closed Workers. Page-owner tests
verify that the MTS realm survives until the reply. Observer tests cover plain
object behavior, target liveness, one-shot cleanup and shared checkpoints.

References in `lynx-stack`: `web-core/ts/client/mainthread/Background.ts`,
`background/background-apis/crossThreadHandlers/registerDisposeHandler.ts`,
`createJSObjectDestructionObserver.ts`, and React's `core/main-thread-ref.ts`.
