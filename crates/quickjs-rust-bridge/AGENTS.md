# crates/quickjs-rust-bridge

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/quickjs-rust-bridge`. Paths in backticks are relative to the repository root.

An owner-thread-bound safe Rust wrapper around the pinned `vendor/quickjs`
submodule. It exposes QuickJS's two objects as two types: a `Runtime` (heap,
atom table, job queue, execution limits, registered module source) and the
`Context` realms created on it, as many as the host wants, all on the owning
thread. Realms share what the runtime owns and nothing else — a `Value` never
crosses between them, one registered module source compiles into a separate
instance per realm, native host modules are installed per realm under one
runtime-wide specifier namespace, and a *failure* belongs to a realm even
though the queue it came out of does not: a pending-job drain names the realm
it reports for, runs every queued job whichever realm queued it, and reports
only that realm's unhandled rejections. A sibling's stays queued for the
sibling's own next drain and is freed with that realm; a caller that has
reported one realm's failure can drop what that realm still has queued, since
one throw rejects a module's evaluation promise and everything awaiting it.

It owns the QuickJS C build and the narrow unsafe FFI shim, realm/value
lifetime and affinity checks, exact ECMAScript string conversion, exception
sanitization, pending-job pump, synchronous preloaded source/native-module
loader, loaded-module namespace access, and module-evaluation Promise state,
and one root-module load (`Context::load_module`): QuickJS's `JS_LoadModule`
of one name with the name as its own base, which evaluates a graph whose
sources the realm has under the call and otherwise defers the load as an
`import()` does, answering the promise the load settles.
It also owns one synchronous load-and-compile entry, which is what a realm's
`require` is written over: `register_synchronous_loader` exports a
`loadModuleSync(url, parameters)` on a native module, backed by a host `FnMut`
that answers one URL at a time. The bridge holds only the mechanism — it
resolves nothing, caches nothing, and stays ignorant of URLs and media types,
which of CommonJS, JSON and an ES module a source is read as being the host's
answer beside the response URL and the text, or, where the host declines to
say, QuickJS's own syntax detection over that text. What it does own is the
order: the response URL is copied and the text compiled (under that URL, in a
wrapper of the caller's parameter list), JSON-parsed, or copied into the
realm's own source table and compiled as a module *before* anything of the
file is evaluated, which is the first point author code can run and so the
first point another load can replace the buffers the host lent. A module is
linked during that compile, so the imports it pulls in are loaded — each
through the same borrowed-until-the-next-load host callback — before its own
evaluation starts. A module whose evaluation throws fails the load with that
throw and leaves no rejection behind: QuickJS runs every module body as an
async function, so the throw rejects that body's promise, each failing
module's on the path and the evaluation promise, and the shim forgets every
rejection the evaluation recorded with the thrown value
(`qjs_rejections_forget_since`), so a caller that catches it has handled it.

**Bytecode of host-loaded scripts goes out and comes back in**
(`docs/resource-cache-design.md`, "PR2"). The bridge compiles in two places
for text a host fetched — a realm-local module when it is first linked, and a
synchronous load — and each can keep the bytecode of its compile or read
bytecode instead of compiling. `Context::complete_module_with(name, url,
text, ModuleCode)` is `complete_module` with the choice: `Compile` (what
`complete_module` does), `Capture` (compile, and queue the bytecode for
`Context::take_compiled`), `Cached(bytes)` (read the bytes; if they are not
this module, compile the text and capture), `Bytecode(bytes)` (read the bytes
and nothing else: the module has no text, and bytes outside this build's
frame, bytes `JS_ReadObject` refuses, a value that is not a module, or a
module compiled under another name fail every import of the name with a
`TypeError` naming it — the shim marks such a source `strict` and keeps the
refusal as its error — rather than falling back). `Context::compile_module_framed`
is `compile_module_bytecode` with the frame added, which is what `Cached` and
`Bytecode` read, and `is_framed_bytecode` is the frame check alone, for a
caller that refuses bad bytes where it is handed them. The synchronous loader's
counterpart is `register_synchronous_loader_with_code`, whose host callback is
also told the wrapper's parameter list and answers a `RequiredCode { source,
cached, capture }`; JSON is never read from bytecode nor captured. A
`Compiled { kind, name, bytecode }` names what the compile baked in: a
module's name, or a CommonJS wrapper's response URL with its parameter list
(`CompiledKind::CommonJs`). The bytes are the bridge's own framing — its
`bytecode_engine_id()` and the payload length ahead of `JS_WriteObject`'s
output — and a caller stores and returns them whole. What the rules are:

- **A read never fails a load**, except a `Bytecode` completion's, which has
  no text to fall back to. Bytes with another engine id or length, bytes
  `JS_ReadObject` rejects, a value of the wrong kind, and a module read under
  another name all fall back to the text with capture on, leaving no pending
  exception. An import that cannot be loaded is not such a failure: the read
  module's import list is the text's own, so it fails exactly as the text
  does.
- **The frame is a guard, not a checksum.** `JS_ReadObject` trusts its input:
  it reads what another build wrote as if this one had. The frame turns away
  truncated bytes and another build's before any read; bytes damaged in place
  are the caller's to detect (`vant`'s stored entries carry a CRC-64). A read
  that does fail part-way through a module leaves nothing behind: the fork's
  `JS_ReadModule` releases both of the module's references on failure, which
  frees it and takes it off the realm's loaded-module list, and a module that
  was read whole and then refused is released the same way
  (`qjs_drop_read_module`).
- **A capture is the bytes of an unlinked module** whether the compile was
  linked or not (`JS_WriteObject` writes names, never resolutions), so one
  capture serves an `import` and a `require` alike. `import.meta.url` is set
  after the compile or the read and is no part of the bytes.
- `bytecode_engine_id()` hashes a build-time digest of the QuickJS and shim
  sources (`build.rs`, `QJS_ENGINE_SOURCES_DIGEST`), the crate version,
  `BC_VERSION` as a scratch `JS_WriteObject` reports it, pointer width and
  byte order. Whether a runtime strips source text is not in it:
  `Context::strips_source` reports that, and a caller that caches keys on it.

A runtime can also reserve module-name prefixes
(`Runtime::reserve_module_prefix`), which checks a name at two places. The
loader looks a name up in the realm's own sources, the runtime's registered
sources, the pending loads and the realm's native modules; a reserved name
found in none of them fails the import in that realm with a `ReferenceError`
instead of becoming a module request or a call of the synchronous loader.
`loadModuleSync` makes the same check itself, after its lookup of a module the
realm already has an instance of, because a top-level `require` does not pass
through the loader. Which prefixes are reserved is the host's choice.

Every heap allocation made by the C shim or the five compiled QuickJS C
translation units is redirected through a private C ABI into Rust's global
allocator; a fixed aligned prefix supplies the size required for matching
`realloc`/`free` and QuickJS memory accounting. QuickJS's `snprintf` and
`vsnprintf` calls are likewise redirected to a crate-private wrapper around the
pinned, allocator-free `nanoprintf` header, so native and Wasm builds share one
integer/string formatter without importing libc `stdio`, `FILE`, locale, or
another heap. All targets compile the C sources against the same crate-private
`stdlib`/`stdio`/`inttypes`/`string`/`math` declaration facade: host allocation
and the audited C gaps route to Rust, stack and basic memory operations remain
compiler builtins, and the bridge-unexposed `FILE`/standard-stream diagnostic
API is compiled out rather than modelled as a platform ABI.

The realm deliberately does not install JavaScript shared-memory primitives:
both `Atomics` and `SharedArrayBuffer` are absent, while ordinary
`ArrayBuffer`, typed arrays, and `DataView` remain available. This does not
disable Rust-side atomics used for interruption or host synchronization.
Because QuickJS formerly coupled its process-global class-ID mutex to the same
feature, the bridge allocates its three class IDs (host-function owner,
`ShadowRealm` instance, realm link) in one Rust `OnceLock` initializer and
registers them separately in each runtime, preserving concurrent native realm
creation.

A realm can also carry the TC39 **`ShadowRealm`** built-in
(`Context::install_shadow_realm`), implemented in the shim over public QuickJS
API only. `new ShadowRealm()` makes a fresh child context on the same runtime
and hands it, synchronously, to the host's hook (HostInitializeShadowRealm) as
a `Context` of its own: the host owns every child, installs whatever it should
have, and keeps it for as long as it should live; an `Err` from the hook is a
`TypeError` in the parent. `evaluate` is the child's own `%eval%` called
indirectly (a fresh lexical environment per call, after a compile-only parse
so a parse failure is a `SyntaxError` and anything thrown a `TypeError`), and
`importValue` is the child's own module load, deferred through the child's
loader like any import of its. Values cross only as primitives and wrapped
functions; every boundary error is a fresh `TypeError` of the realm whose code
reached the boundary, carrying the original's `message` only when it is an
`Error` with a string data property, and an interrupt stays uncatchable on the
far side. JavaScript holds a child only through a C-counted **token**, never
an untraced `JS_DupContext`: its context goes NULL as the first step of
`qjs_context_free`, so a released realm is a `TypeError` at every crossing
rather than memory something points at, and
`Context::is_referenced_across_realms` reports whether a `ShadowRealm`, a
wrapped function or a pending `importValue` still needs the realm.
No `Value` crosses realms in Rust; a child's code runs under the parent
entry's execution limits.

**One JS value is shared across realms by reference: the runtime's shared
object.** `Context::register_host_module_shared_object(module, export)` adds
one null-prototype object as a host-module export, and every realm of the
runtime it is added to gets the same reference, so what one realm stores on
it — functions included — another reads and calls as itself, with no host
function in between. The shim keeps it while any realm it was added to is
alive and counts those realms down in `qjs_context_free`; the next realm it
is added to after the last of them gets a new, empty object. A function one
realm stored there and another calls runs in the realm that made it, inside
the caller's entry. `bobcat-core` adds it to every worker realm as
`bobcat-internal:worker`'s `sharedObject`, and nowhere else.

It also owns the **host-function seam**: `Realm::function`,
`define_global_function`, and `register_host_module_function` back a JS
callable with a Rust `FnMut`, dispatched through one C trampoline
(`JS_NewCFunctionData` + a realm-owned callback table reached via the context
opaque). Host callbacks speak `HostArgument` in and `HostValue` out:
undefined/null/bool/number/string, an opaque structured clone
(`JS_WriteObject`'s bytes) for any other value the serializer accepts, and
the bytes of a bare `ArrayBuffer` (below). Functions, symbols, and
ill-formed UTF-16 strings are rejected on the way in rather than lossily
converted, element identity crosses as plain numbers, and handle objects
never leave JavaScript. Their `FnMut` closure is borrowed through a `RefCell`, so reentry
is refused rather than aliasing it; `function_reentrant` and
`register_host_module_function_reentrant` take an `Fn` closure instead, which a
nested call runs again (`bobcat-core`'s `dispatchEvent` is the one user). A
`Context` is an `Rc` handle whose state is all interior, so the methods a
call into a realm uses (`evaluate`, `module_namespace`, `member`,
`call_member`, the module-loading members and the host-function
registrations) take `&self`: a host function may call into any realm of the
runtime, its own included, while a call further up the stack holds that
realm. A panicking callback becomes a JS exception and leaves the slot usable.

**`ArrayBuffer` bytes cross as `HostArgument::Bytes(&[u8])` /
`HostValue::Bytes(Box<[u8]>)`.** The rule (`qjs_host_is_fixed_array_buffer`
in `shim.c`): an argument that is itself an object of QuickJS's
`ArrayBuffer` class (a subclass instance included) and not resizable. A
typed array, a `DataView`, an `ArrayBuffer` inside another value and a
resizable `ArrayBuffer` stay `Structured`, as before. A caller that means a
view's bytes passes `view.buffer` with `byteOffset`/`byteLength` as further
arguments. The class ID, which `quickjs.h` does not export, and each realm's
intrinsic `resizable` getter are read in `qjs_context_new` before any code of
the realm runs, so no getter the realm's code installed runs during
describing.

- **Inbound is one copy.** The trampoline copies the buffer into a
  `js_malloc`'d block it frees after the dispatch; the slice borrows that
  block, never the buffer's storage. Any host function, not only a reentrant
  one, can call into its realm (`evaluate`, `call_member`, a getter through
  `member`) while it holds the slice, and that code can write the buffer
  through a view, detach it (`transfer`) or free its storage (`transfer` to
  another length, which frees through the buffer's free function, or
  `resize`, which `js_realloc`s). A borrow of the storage would not survive
  any of these. `to_value` copies again into an owned box.
- **A detached buffer** is the `TypeError` `JS_GetArrayBuffer` throws, at the
  call, and the host function is not called: the same refusal
  `JS_WriteObject` makes of one. An empty buffer is `Bytes(&[])`.
- **A returned `HostValue::Bytes` is adopted, not copied**: `write_result`
  hands the box to `qjs_host_build`, which makes it the storage of a new
  `ArrayBuffer` with `JS_NewArrayBuffer` and `qjs_rust_bytes_free`
  (`allocator.rs`) as its free function, the box's length in the opaque.
  QuickJS frees through it on detach, on a transfer to another length and in
  the finalizer; the finalizer of a detached buffer passes null, which it
  ignores. Such a buffer is not resizable (`js_array_buffer_resize` refuses
  an external one), and its storage is not counted against the runtime's
  memory limit (`JS_ComputeMemoryUsage` does count it). An empty box becomes
  an ordinary empty buffer.
- **Bytes lent into a call** (`HostArgument::Bytes` through `call_member`, or
  `HostValue::as_argument`) are copied into a new `ArrayBuffer`
  (`JS_NewArrayBufferCopy`). So a bare `ArrayBuffer` posted to another realm
  arrives as an `ArrayBuffer` with equal bytes, fixed-length.
- `SharedArrayBuffer` is not exposed to JavaScript (`globalThis` has none),
  and its class is not `ArrayBuffer`'s, so it never crosses as bytes.

**A message can be written with a transfer list, and read with one
replacement per listed object.** The fork adds three C functions
(`quickjs.h`): `JS_SetUncloneable(ctx, obj, transferable)` sets two bits on
the object (`is_uncloneable`, `is_transferable`, in the byte `is_HTMLDDA`
uses, so `JSObject` does not grow); `JS_WriteObjectTransfer(ctx, &len, obj,
flags, transfer, n)` writes as `JS_WriteObject` does, except that each
`transfer[i]` is written as the tag `BC_TAG_TRANSFER` and the index `i`,
wherever and however often `obj` holds it; `JS_ReadObjectTransfer(ctx, buf,
len, flags, values, n)` reads that tag as `values[i]`, and an index `>= n` is
a `SyntaxError`. `JS_WriteObject`/`JS_ReadObject` are the same functions with
`n = 0`.

- **An uncloneable object is refused by every write it is not listed in**:
  `TypeError: uncloneable object`, from `JS_WriteObject` (every structured
  argument of every host function) and from `JS_WriteObjectTransfer` alike.
  The writer tests the bit once per object, before the object reference
  lookup, and searches the list only for a marked object; a value with no
  marked object costs one bit test per object, with or without a list.
- **The list is checked before anything is written**: every entry must be an
  object marked with `transferable` true and must appear once, else
  `TypeError: value is not transferable` or `TypeError: duplicate
  transferable`. Marking an object again with `transferable` false keeps it
  uncloneable and keeps it out of any list (an object whose host state has
  moved out). These three messages are the refusals that HTML calls a
  `DataCloneError`; the bridge does not know `DOMException`, so the layer
  above maps them by message.
- **Slot `i` is list entry `i`**, whether or not the message holds it, so a
  stream has as many slots as the list has entries. A listed object is not
  added to the writer's object reference list and a slot adds nothing to the
  reader's object table, so reference indices agree on both sides; the
  positions that held one listed object read as one value.

The bridge exposes this in three pieces:

- `Context::register_host_module_function_with_transfer(module, export,
  arity, message_index, f)` (and `function_with_transfer`) makes a host
  function with its own C trampoline, `qjs_host_transfer_trampoline`; the
  trampoline every other host function uses is unchanged. Argument
  `message_index` is the message and `message_index + 1` its list:
  `undefined` or an array, else a `TypeError`. The list is read first (an
  array read, which can run the realm's code). With an empty list the message
  is described as any argument is (`Structured`, `Bytes` or a primitive); with
  a non-empty one it is written by `JS_WriteObjectTransfer` and handed to `f`
  as `HostArgument::Transferred(TransferredBytes)` with `slots()` equal to
  the list's length, whatever the message is. The list's own argument is
  handed as `HostArgument::Number(length)`. A refusal throws at the call and
  `f` is not called. The trampoline changes nothing in the realm: which
  object each slot was is known to the realm's own code, which passes the
  embedder's ids as further primitive arguments in list order, and moving
  the host state out is `f`'s, after the write succeeded. `message_index` is
  at most 253 (`QuickJS` keeps a function's length in eight bits).
- `Context::register_host_module_transfer_members(module, mark, reviver)`
  adds two C functions as exports, neither of which crosses into Rust:
  `mark(object, transferable)` is `JS_SetUncloneable`, and
  `reviver(fn)` stores, once per realm, the function a delivery calls per
  slot. A second `reviver` call is a `TypeError`.
- Delivery: `Revival::new(transferred, &descriptors)` pairs a stream with one
  primitive (or structured) `HostArgument` per slot, and
  `HostArgument::Revive(&revival)` is lent into any call (`call_member`, a
  host-module export call). The realm builds each descriptor, calls its
  reviver with it in slot order, and reads the stream with the answers as the
  slots' values. No reviver, a reviver that throws, or a descriptor that
  cannot be built fails the call before the stream is read. A
  `HostArgument::Transferred` lent as it is reads with no replacements, so a
  stream with slots fails at its first slot (`SyntaxError: invalid transfer
  slot`).

**The stream is never visible to JavaScript as bytes.** It exists only in a
`TransferredBytes`/`TransferredClone`, which have no byte accessor, and is
read only by `JS_ReadObjectTransfer` inside the shim, so a script cannot hand
the reader a stream it made. `TransferredBytes` is a pointer and two `u32`s
so that `HostArgument` stays 24 bytes and `HostValue` 32 (a static assertion
holds both; one more word on either slowed every host call, the primitive
ones included, in `benches/host_calls.rs`), and the trampoline refuses a
stream longer than `u32::MAX` bytes with a `RangeError`.

A closure's lifetime follows its JS function object rather than the realm: the
closure sits at its own stable heap address, which a companion JS object holds
and the collector hands back through a finalizer, so nothing is indexed,
recycled, or aliasable by a stale reference, and discarding a function drops
its closure. Without this a realm registering a handler per element per update
(events, worklets) would accumulate every closure it ever made. The finalizer
only *records* the address; the drop happens at the next `&mut Realm` entry
point, because a handler may own a `Value` whose `Drop` calls `JS_FreeValue`
and re-entering QuickJS from inside its own GC is unsound. Capturing a
same-realm `Value` is therefore safe, but forms a reference cycle that leaks
the realm unless the function is collected first. The same holds one level up
for `ShadowRealm` children: a child `Context` is freed when the host drops its
last handle, so a child kept by its parent's hook is freed with the parent,
while a child kept from a closure the child itself owns (one of its own host
functions, or its own hook) is a cycle that leaks it. The token counts that
`is_referenced_across_realms` reads are updated by finalizers, so a
`ShadowRealm` or wrapper that is only cyclic garbage still counts until
`Runtime::run_gc` has collected it. The crate must remain
independent of Bobcat, the DOM, resources, and runtime policy — it knows
nothing about Lynx.
