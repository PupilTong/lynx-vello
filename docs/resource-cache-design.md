# Resource loading and caching: the `vant` policy crate

Status: design accepted 2026-11-10 (owner ruling). Series of independent PRs,
each based on `main`; this document is the standing spec for all of them and
is updated as each lands.

## Why

`crates/bobcat-resources` today is one 10.7k-line crate holding two different
things: **policy** that every embedder needs identically (the memory bitmap
tier, RFC 9111 freshness, image residency and draw-sized refinement, the
stylesheet in-flight cache, MIME sniffing and preprocessing, the registry) and
**platform adapters** that only one target needs (libcurl, `file:` mapping, a
cache directory, ImageIO, gdk-pixbuf, a tokio runtime; or the Render Worker's
`fetch` and the main thread's `Image` element). A new embedder — iOS, Android,
HarmonyOS, a test host — either links the whole crate or rewrites the policy.

The 2026-08-07 embedder-boundary ruling says an embedder provides *IO atomic
capabilities* (bundle bytes, resource fetch, file write) and nothing of the
engine's scheduling or policy. The split below applies that ruling to
resources: the engine owns the policy, the embedder owns the bytes.

## The three crates after the series

| crate | owns | depends on |
|---|---|---|
| `vant` | every resource **policy**: `CachePolicy`, the memory tier, RFC 9111 freshness, the byte-cache tier over a `Store`, image residency and refinement decisions, MIME sniffing and preprocessing, image header probing, `data:` URLs, the resource error vocabulary, the embedder traits (`Fetch`, `Store`, `AsyncStore`, `ImageDecoder`) and `Platform`, and (PR2) the derived-artifact cache that backs the QuickJS code cache, with a synchronous and an asynchronous driver | `bytes`, `http`, `url`, `rustc-hash`, `serde_json`, `thiserror`, `vello_common` (for `peniko::ImageData`), `siphasher`. **Never** `terminus`, `bobcat-core`, tokio, libc bindings or `web-sys`. |
| `bobcat-resources` | the reference **orchestrator** (`Resources<P>`, `ViewResources<P>`, the image job pipeline, source loading, background installs over `vant::registry` (PR4), the native executor) and the shipped **native adapters**: `NativeFetch` (libcurl + `file:`), `FsStore` (a cache directory), `NativeDecoder` (ImageIO / gdk-pixbuf); the browser's adapters (`BrowserFetch`, `BrowserDecoder`, `CacheStore`) are `bobcat-wasm`'s since PR3 | `vant`, `bobcat-core`, tokio, the platform libraries |
| `terminus` | re-exports `vant::ImageSizeHint` at `terminus::link::ImageSizeHint`; (PR5) owns the image store type `terminus::link::ImageResidency` over `vant::image::Residency`, with the engine's constants and report dispatch | `vant` |

Dependency direction: `terminus → vant`, `bobcat-core → terminus, vant`,
`bobcat-source → bobcat-core, vant` (registrations are written through
`vant::registry` since PR4; `bobcat-resources` is only its dev-dependency),
`bobcat-resources → vant, bobcat-core`. `vant` sits below `terminus`, so nothing in it may name a
DOM, style or runtime type; what the pipeline needs from above arrives as a
generic parameter or a trait.

## The embedder traits (in `vant`)

Modelled on the web platform's Fetch and Cache APIs, with the additions the
Cache API lacks and this engine needs. Capabilities are traits with associated
types, never `dyn` (2026-08-09 ruling). On native targets every trait requires
`Send + Sync + 'static`; on wasm32 none does, because the browser decoder holds
a `MessagePort` and the system never leaves the Render Worker. The crate
expresses that with one cfg'd marker supertrait, `vant::Threaded`.

### `Fetch` — the transport

```rust
pub struct Request {
    pub url: Url,
    pub policy: CachePolicy,
    /// Caller headers plus any conditional headers the byte tier added.
    pub headers: HeaderMap,
    pub timeout: Duration,
    pub max_body: usize,
    pub max_redirects: u32,
    pub user_agent: String,
}

pub struct Response {
    /// The final URL after redirects.
    pub url: Url,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub trait Fetch: Threaded {
    /// Moves the bytes, blocking the calling thread. `None` from a transport
    /// that cannot block (the browser), which makes a synchronous restore
    /// impossible and tells the image pipeline to retain encoded bytes.
    fn fetch_blocking(&self, request: &Request) -> Option<Result<Response, Failure>>;

    /// Moves the bytes asynchronously. The default answers with
    /// `fetch_blocking`; the browser overrides it with `fetch()`.
    fn fetch(&self, request: Request) -> impl Future<Output = Result<Response, Failure>>;

    /// Whether the transport applies HTTP cache semantics itself (the
    /// browser's `fetch` does, through the request's cache mode). When
    /// `true`, `vant` runs no byte tier in front of it.
    fn owns_http_cache(&self) -> bool;

    /// Reads a `file:` URL's contents, mapped read-only, for bytes a consumer
    /// retains for a view's lifetime (fonts). `None` where files cannot be
    /// mapped. Mapped bytes come back as the same `Bytes` for one unchanged
    /// file, process-wide.
    fn map_file(&self, url: &Url) -> Option<Result<Bytes, Failure>> { None }
}
```

A `Fetch` serves `http`, `https` and natively `file:`. Registered contents and
`data:` URLs are answered by `vant` before any transport is asked.

### `Store` — persistent bytes

Synchronous: natively a directory. The HTTP byte tier needs synchronous
reads (a restore inside `FrameImages::read`, a mapped body), so it is always
built on a `Store`. A browser has no synchronous storage every scope can use,
so its only persistent tier, the derived-artifact tier, is built on an
`AsyncStore` instead (below).

```rust
/// What a `Store` keeps beside a body. `record` is the tier's own
/// serialisation (an HTTP response record, an artifact header); the Store
/// never reads it.
pub struct Meta {
    /// The full key, so a Store that names entries by a hash can detect a
    /// collision: `vant` checks it on every read and treats a mismatch as a
    /// miss.
    pub key: String,
    pub len: u64,
    pub last_used: SystemTime,
    pub record: serde_json::Value,
}

pub struct Estimate { pub used: u64, pub quota: Option<u64> }

/// How the host may take the Store's contents away.
pub enum Volatility {
    /// Only this engine deletes entries.
    Durable,
    /// The OS may purge everything while no process of the app runs (iOS
    /// `Library/Caches`, macOS caches).
    PurgedWhenIdle,
    /// Entries may disappear at any moment, singly or all at once (Android
    /// `cacheDir`, browser storage under pressure or Safari's 7-day cap).
    EvictableAnytime,
}

pub trait Store: Threaded {
    /// Every entry present, for rebuilding the index. Broken halves are
    /// deleted on the way.
    fn scan(&self) -> Result<Vec<Meta>, StoreError>;
    fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError>;
    fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError>;
    /// Atomic per entry: a crash leaves the old entry or none, never a
    /// truncated one.
    fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError>;
    /// Replaces the record of an existing entry (a 304 refresh).
    fn put_meta(&self, meta: &Meta) -> Result<(), StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// The LRU signal; an implementation also moves the file's mtime so a
    /// host that evicts by mtime (Android) agrees with `vant`'s order.
    fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError>;
    /// The body mapped read-only, for retained bytes; `None` where not
    /// supported.
    fn map_body(&self, key: &str) -> Option<Bytes> { None }
    fn estimate(&self) -> Estimate;
    fn volatility(&self) -> Volatility;
}
```

Rules every tier built on a `Store` follows:

- The in-memory index is rebuilt from `scan` at open and is a hint afterwards:
  a `get` that fails or returns a `Meta` whose `key` differs is a miss and a
  `delete`.
- Budget is `min(configured, estimate().quota)` when a quota is known.
- Writes are content-complete and single-shot (`put`); nothing is appended.
- `touch` is rate-limited by the tier (once per 24 h per entry) so a hot
  entry costs no write per hit.
- Trimming (LRU by `last_used`) runs at open and whenever a `put` crosses the
  budget; an entry larger than the whole budget is refused.

### `AsyncStore` — persistent bytes, answered asynchronously

Modelled on the web Cache API (`caches.open(name)`), with only what the
derived-artifact tier uses. The contracts are `Store`'s: `put` is atomic per
entry, the tier checks `Meta.key` on every read, and entries may vanish at
any time.

```rust
/// `Send` on native targets, nothing on wasm32.
pub trait MaybeSend {}

pub trait AsyncStore: Threaded {
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + MaybeSend;
    fn get(&self, key: &str)
        -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + MaybeSend;
    fn put(&self, meta: &Meta, body: Bytes)
        -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn touch(&self, key: &str, now: SystemTime)
        -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn estimate(&self) -> impl Future<Output = Estimate> + MaybeSend;
    fn volatility(&self) -> Volatility;
}
```

`NoStore` implements both `Store` and `AsyncStore`.

### `ImageDecoder` — platform pixels

```rust
pub trait ImageDecoder: Threaded {
    /// Decodes at most `max` large, blocking. A decoder that cannot block
    /// (the browser's main-thread `Image`, outside the mailbox path)
    /// answers `DecodeError::Unavailable`.
    fn decode_blocking(&self, bytes: &[u8], format: ImageFormat,
                       header: Option<ImageHeader>, max: (u32, u32))
        -> Result<Bitmap, DecodeError>;
    fn decode(&self, bytes: Bytes, format: ImageFormat,
              header: Option<ImageHeader>, max: (u32, u32))
        -> impl Future<Output = Result<Bitmap, DecodeError>>;
    /// Reported once at startup rather than once per image.
    fn available(&self) -> Result<(), DecodeError>;
}
```

`Bitmap`, `DecodeError`, `ImageFormat`, `ImageHeader` and `ImageSizeHint` all
live in `vant`.

### `Platform` — the bundle an embedder hands over

```rust
pub trait Platform: 'static {
    type Fetch: Fetch;
    type Store: Store;            // `NoStore` for a host with no synchronous tier
    type AsyncStore: AsyncStore;  // `NoStore` for a host without one
    type Decoder: ImageDecoder;
    /// Taken apart once, when the resource system is built: each tier owns
    /// its store.
    fn into_parts(self) -> Parts<Self::Fetch, Self::Store, Self::AsyncStore, Self::Decoder>;
}
pub enum ArtifactStore<S, A> { Blocking(S), Async(A) }
pub struct Parts<F, S, A, D> {
    pub fetch: F,
    pub decoder: D,
    pub bytes: Option<Tier<S>>,                          // the HTTP byte tier
    pub artifacts: Option<Tier<ArtifactStore<S, A>>>,    // the derived-artifact tier (PR2)
}
/// A Store with the budget of the tier over it; the tier clamps it to the
/// Store's quota. `u64::MAX` means "the quota, or unbounded".
pub struct Tier<S> { pub store: S, pub budget: u64 }
```

The resource system (`Resources::with_platform`) takes the parts, opens a
`ByteCache` over `bytes` unless the `Fetch` owns its HTTP cache, and an
`ArtifactCache` (over a `Store`) or an `AsyncArtifactCache` (over an
`AsyncStore`) over `artifacts` whatever the transport. (PR1 shipped
`into_parts` as a tuple with a `byte_budget` method; PR2 gave it this shape
when the second tier arrived.)

`bobcat-resources` ships `NativePlatform` (`NativeFetch`, `FsStore`,
`NativeDecoder`, `ArtifactStore::Blocking(FsStore)` for the artifact tier);
`bobcat-wasm` ships `BrowserPlatform` (`BrowserFetch`,
`ArtifactStore::Async(CacheStore)` for the artifact tier, `BrowserDecoder`). An embedder with its own HTTP stack or storage implements
the traits and passes its own `Platform`.

## Tiers in `vant`

- `vant::memory::MemoryCache<V>` — unchanged from today's memory tier: LRU by
  monotonic stamp under a byte budget, with a pinned working set.
- `vant::http` — unchanged RFC 9111 computation (`StoredResponse`, `Freshness`,
  `Plan`, `plan`), plus the JSON (de)serialisation of a `StoredResponse` for a
  `Meta::record`.
- `vant::byte_cache::ByteCache<S: Store>` — the HTTP byte tier: what
  `cache/disk.rs` and the cache half of `Transports::http_blocking` did,
  generic over the Store. API: `open(store, budget)`, `lookup(key, policy, now,
  read) -> Lookup` (`UseStored(Entry, Bytes)` / `Fetch { conditional:
  Option<HeaderMap>, revalidating: bool, store: bool }` / `Unavailable`), where
  `Read::{Owned, Mapped}` says how the body is handed over (`Mapped` asks
  `Store::map_body` first and verifies the mapping against `get_meta`) and
  `Entry` is the index's view (`Meta`, media type, `StoredResponse`);
  `store(key, media_type, &Response, now) -> bool`, `refreshed(key,
  &not_modified_headers, now, read) -> Option<(Entry, Bytes)>`,
  `map_body(key)`, `entry(key)`, `remove`, `clear`, `used_bytes`, `len`,
  `budget`, `store_ref`.
- `vant::image` — `ImageSizeHint`, `Bitmap`, `DecodeError`, `target_size`,
  `wants_refinement`, `bounded` (against a `max_renderable` the caller passes;
  `vant` does not know the GPU limit).
- (PR2) `vant::artifact::ArtifactCache<S>` and `AsyncArtifactCache<A>` — see
  "PR2: QuickJS code cache" and "PR3: browser code cache store".

## PR1: `vant` exists, embedders provide the platform

1. New crate `crates/vant`. Moved verbatim (tests included), with doc paths
   updated: `cache/memory.rs → memory.rs`, `cache/http.rs → http.rs`,
   `mime.rs`, `preprocess.rs`, `image_header.rs`, `data_url.rs`,
   `CachePolicy`; from `terminus`: `ImageSizeHint` (with its tests) into
   `vant::image`; from `bobcat-core::resource`: `ResourceError`,
   `ResourceErrorKind`, `ResourceErrorPhase`, `RetryAdvice` into
   `vant::error`, and `bobcat-resources`' `Failure` beside them, now `pub`.
   `terminus::link::ImageSizeHint` and `bobcat_core::resource::{ResourceError,
   …}` become re-exports, so no caller outside the three crates changes.
   `Bitmap`, `DecodeError`, `target_size` move from `bobcat-resources::decode`
   into `vant::image`; `wants_refinement` and `bounded` move from
   `images.rs`.
2. The traits above, `Threaded`, `NoStore`, and `ByteCache<S>` (from
   `disk.rs`'s index/LRU/verification logic) in `vant`, with the disk tests
   rewritten against an in-memory test `Store`.
3. `bobcat-resources`: `FsStore` (today's file layout: `<fnv64>.bin` +
   `<fnv64>.json`, temp+rename, sweep at open, `default_cache_dir`),
   `NativeFetch` over `transport/curl.rs` and `transport/file.rs`,
   `NativeDecoder` over `decode/{imageio,gdk_pixbuf}.rs`, `BrowserFetch` over
   `transport/browser.rs`, `BrowserDecoder` over `decode/browser.rs`;
   `Resources<P: Platform>`, `ViewResources<P>`, `Shared<P>`;
   `Resources::with_platform(config, platform, notes, wakeup)`, and
   `Resources::new(config, wakeup)` over the target's `DefaultPlatform`
   (`NativePlatform` / `BrowserPlatform`, no persistent store) so that every
   `LynxView<ViewResources>` signature keeps compiling through the default
   type parameter; `ResourcesConfig` loses `disk_cache` and `image_port`
   (they are the platform's). `Transports` becomes the dispatch over registry
   → `data:` → `ByteCache`/`Fetch`.
   Behaviour is unchanged: the same tiers, the same policies, the same
   notes.
4. Embedders: `bobcat-cli` builds `NativePlatform::new(Some(&disk))`,
   `bobcat-server` keeps `Resources::new` (no disk cache, as before),
   `bobcat-wasm` builds `BrowserPlatform::new(Some(image_port))`.
   `LynxView<ViewResources>` is `LynxView<ViewResources<DefaultPlatform>>`.
5. CI: `-p vant` joins the wasm32 clippy list; AGENTS.md's workspace map and
   the `bobcat-resources` section describe the split; this document is the
   design reference.

Not in PR1: the code cache (PR2), the browser Store (PR3), the registry
(PR4), image residency (PR5).

## PR2: QuickJS code cache

Three parties, each doing the one thing only it can:

- **The engine (`bobcat-core`) looks bytecode up at the moment it would
  compile**, synchronously, in `vant::artifact::Artifacts` — a process-wide
  memory tier (`Arc`, `Send + Sync` on every target, bounded by bytes, LRU).
  A `require`/`loadModuleSync` must answer within the call and the browser's
  storage cannot be read from a realm's thread, so the lookup the engine makes
  is always a memory lookup. On a miss it compiles as before and asks the
  bridge to keep the bytecode of that compile, which it then inserts.
- **The resource system (`bobcat-resources`) owns the embedder's `Store`.**
  Before it delivers a script's text it hashes the text and `preload`s every
  stored variant of that source into the memory tier
  (`ArtifactCache::preload`); after the engine compiled something new it
  writes it back (`ArtifactCache::flush`, from the pump, on the blocking pool
  natively). `LoadedSource::Module` is unchanged: nothing about bytecode
  crosses the fetcher protocol, so a fetcher that knows nothing of this still
  gets in-process reuse.
- **The bridge (`quickjs-rust-bridge`) reads and writes.** `ModuleCode::{Compile,
  Capture, Cached}` on `Context::complete_module_with`, `RequiredCode` on the
  synchronous loader, `Context::take_compiled` for what a capture left. Bytecode
  that fails to read falls back to the text and re-captures; `JS_ReadObject`
  trusts its input, so nothing reaches it that `vant::artifact::decode` (for a
  stored entry) or this process's own compile (for a resident one) did not
  vouch for.

Key: `ArtifactKey { source: SourceDigest, variant: Variant }` under an
`EngineId`.

- `SourceDigest` — 128-bit SipHash-1-3 of the text. `.web.bundle` headers
  carry no content hash, so the text itself is hashed.
- `Variant` — what the compile bakes in beyond the text: for a module its
  name (stack frames and the module record carry it; `import.meta.url` is set
  after the read and is not part of it), for a CommonJS file the response URL
  it is compiled under and the wrapper's parameter list; plus whether the
  runtime strips source text.
- `EngineId` — the bridge's `bytecode_engine_id()`: a build-time digest of the
  QuickJS and shim sources, the bridge version, `BC_VERSION`, pointer width
  and endianness.

Stored entry, verified by `decode` before the payload is handed on:

```
magic "VANTART\0" | format u32 | engine u64 | source u128 | variant u64 |
payload length u64 | crc64 | bytecode
```

Storage: `vant::artifact::ArtifactCache<S: Store>` over a Store of its own
(natively `<cache dir>/code`, beside the byte tier's files). Entries are
content-addressed, so two processes writing one key write identical bytes and
either rename may win; there is no shared index file (Lynx's single
`meta.json` under a process-local mutex is the pattern to avoid). Expiry: 15
days since last use, a byte budget (64 MiB by default, clamped to the Store's
quota), and a wipe of every entry another `EngineId` wrote. Without a Store
the memory tier alone serves, with identical keys, so adding storage later
changes no caller; this is also what gives `bobcat-server` reuse across
captures and several views of one card a single compile.

`Platform::into_parts` hands over `Parts { fetch, decoder, bytes:
Option<Tier<Store>>, artifacts: Option<Tier<ArtifactStore<Store,
AsyncStore>>> }`, each `Tier` a store with its budget.

Sources below 1 KiB are not cached. Not covered: `Context::evaluate` (the
engine runs no fetched text through it) and runtime-registered text modules
(tests and fixtures only).

Pairing a compile with its text. The bridge names a kept compile by what it
was compiled under (a module's name; a wrapper's response URL and parameter
list), and compiles a module when it is first linked, which can be operations
after its text was handed over. The realm keeps, per name, the digest of the
text it handed over and files a compile under that digest only while every
text handed over under the name was the same one; two different texts under
one name file nothing in that realm. Bytecode is therefore never filed under
a text it might not be the compile of, whatever order hand-overs, cache hits
and compiles come in.

Known limit, in the fork: `JS_ReadModule`'s failure path frees one of the two
references a half-read module holds, so the module stays on the context's
loaded-module list. The bridge's frame (engine id and length) and `vant`'s
CRC-64 keep damaged bytes from reaching the reader, and a module read whole
and then refused is released with both references; freeing the object in
the reader's `fail:` path would close it in the fork.

## PR3: browser code cache store

`vant::AsyncStore` and `vant::artifact::AsyncArtifactCache<A: AsyncStore>`:
the artifact tier's policy driven by awaiting the store. The tier's decisions
(what a preload reads, what `open` keeps and deletes, what a flush writes,
what a trim deletes, how the index changes after each IO answer) are IO-free
steps on one index type; `ArtifactCache<S: Store>` runs the IO under the
index lock, `AsyncArtifactCache` releases the lock while it awaits. Calls may
overlap on one thread, and each overlap has one rule (the type's docs state
them): two preloads of one source both read and both admit; a preload does
not read an entry whose put is in flight; a flush skips a key whose put or
delete another call has in flight; a read that verifies is admitted even if
its entry was trimmed meanwhile, and a read that fails deletes the entry only
if the index still lists the indexing it was issued for.

`bobcat-wasm` implements `AsyncStore` over the web Cache API (`CacheStore`,
over `caches.open("bobcat-code-cache")`, opened when the renderer is built).
A key is the request URL `https://bobcat.invalid/<key>`, each path segment
percent-encoded; nothing is fetched from it, and `https` is the scheme
`Cache.put` accepts. An entry is a `Response` whose body is the entry bytes
and whose headers carry the `Meta`: the key, the length, `last_used` in Unix
seconds and the record as percent-encoded JSON. `scan` is `matchAll()` and
reads headers only; `touch` puts the stored body's stream again under a new
`last_used` header. Recency is that header alone, never the order `keys()` or
`matchAll()` answer in. The Cache API is available in a Window and in every
Worker and is shared by every tab of the origin. Without it (a context that
is not secure, a browser that refuses it in private browsing) the code cache
keeps artifacts in memory only, and that is a note, never an error.

`bobcat-resources`' `CodeCache` holds either driver. Over an `AsyncStore` the
store is opened by a task started with the system, and the first call that
needs it awaits that open; a module's preload is awaited after its text is
prepared and before its completion is delivered, and a flush is a task of its
own, at most one at a time. There is no flush when the last scope lets go:
what the engine compiled after the last serviced turn is compiled again next
time. The byte tier stays absent in the browser: its HTTP cache already does
that job and cannot be read synchronously. `BrowserFetch`, `BrowserDecoder`
and `BrowserPlatform` are `bobcat-wasm`'s: `bobcat-resources` keeps the
orchestration and the native adapters.

## PR4: the registry moves into `vant`

`vant::registry`: `Registry<S>` and `Registered<S>` generic over the host's
parsed-stylesheet type, `Registrar<S>`, `RegisterError`, the URL
normalization every registration goes through, and the traits a source
parser writes through — `Register<S>`, `RegisterInBackground<S>`,
`ContainerInstaller<S>`. `bobcat-source` then names `vant` and `bobcat-core`
only, and `bobcat-resources` becomes its dev-dependency. The install holds
(`Installs`, beside the registry in a scope's `Transports`) and the stylesheet
in-flight cache stay in `bobcat-resources`: they are tokio `watch` channels,
which `vant` does not have. `RegisterInBackground::register_in_background`
takes a `Send + 'static` install on every target, because the reference
system's own method always has; `Resources::register` and
`register_style_sheet` stay inherent (they add the per-scope stylesheet-cache
invalidation) and the trait implementations call them.

## PR5: image residency is a policy

The image pipeline's decisions — the state of every source, who waits for
it, the decoded-bitmap tier and its pins, when a read hits, refines or must
restore — move out of `bobcat-resources`' `images.rs` into
`vant::image::Residency<W>`, a synchronous state machine with no IO.
`terminus::link::ImageResidency` is the engine's store type over it, owning
`MAX_RENDERABLE_DIMENSION` and the dispatch of `ImageReports`, so a host
cannot forget or reorder a report. `bobcat-resources` keeps the IO: fetch,
decode, parse, and feeding completions in. One behaviour fix is part of it:
the pinned working set becomes the union of every view's, where today one
view's `retain` replaces the set and unpins another view's bitmaps.

Not in the series — it needs a ruling: making the CPU tier release a bitmap
the GPU `Bitmaps` cache holds, and restore only when both dropped it. Strong
references to a decoded image today are the host tier, the painter's resolved
set, the composed display list and the filter bake list; releasing the CPU
copy needs the frame to carry a descriptor and a weak blob, a pre-acquire
restore each turn and a lease on the GPU entry. It breaks the tested law that
a dropped view's last frame keeps the pixels it read, and adds host policy.

## Platform constraints the design answers

| platform | storage | when it is taken away | consequence |
|---|---|---|---|
| iOS | `Library/Caches` | purged when disk is low (Apple's disk-usage guidance); app extensions share the directory | `PurgedWhenIdle`; index is rebuilt by `scan`; entries are self-describing |
| Android | `getCacheDir`, `getCacheQuotaBytes` | oldest `lastModified` first, any time | `EvictableAnytime`; `touch` moves mtime; budget bounded by the quota |
| macOS / Linux CLI | `~/Library/Caches/bobcat`, `$XDG_CACHE_HOME/bobcat` | user tools; concurrent CLI processes | atomic writes, no shared index |
| browser | Cache API (the code cache's `AsyncStore`), per-origin quota | whole-origin eviction under pressure; Safari clears script-writable storage after 7 days without interaction | `EvictableAnytime`; persistence is best-effort, the browser HTTP cache carries HTTP freshness |
| bobcat-server | none | fresh per capture | `NoStore`; the process-wide artifact memory tier still shares bytecode across captures |
