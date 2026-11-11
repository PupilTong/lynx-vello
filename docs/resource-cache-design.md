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
| `vant` | every resource **policy**: `CachePolicy`, the memory tier, RFC 9111 freshness, the byte-cache tier over a `Store`, image residency and refinement decisions, MIME sniffing and preprocessing, image header probing, `data:` URLs, the resource error vocabulary, the three embedder traits (`Fetch`, `Store`, `ImageDecoder`) and `Platform`, and (PR2) the derived-artifact cache that backs the QuickJS code cache | `bytes`, `http`, `url`, `rustc-hash`, `serde_json`, `thiserror`, `vello_common` (for `peniko::ImageData`), `siphasher`. **Never** `terminus`, `bobcat-core`, tokio, libc bindings or `web-sys`. |
| `bobcat-resources` | the reference **orchestrator** (`Resources<P>`, `ViewResources<P>`, the image job pipeline, source loading, the registry and background installs, the native executor) and the shipped **native adapters**: `NativeFetch` (libcurl + `file:`), `FsStore` (a cache directory), `NativeDecoder` (ImageIO / gdk-pixbuf); on wasm32 `BrowserFetch` and `BrowserDecoder` until PR4 moves them into `bobcat-wasm` | `vant`, `bobcat-core`, tokio, the platform libraries |
| `terminus` | re-exports `vant::ImageSizeHint` at `terminus::link::ImageSizeHint`; (PR3) adopts `vant`'s image residency store in the painter so the CPU bitmap tier and the GPU `Bitmaps` budget are one decision | `vant` |

Dependency direction: `terminus → vant`, `bobcat-core → terminus, vant`,
`bobcat-source → vant` (for `Registrar`/`ContainerInstaller`, which stay in
`bobcat-resources` until PR3 moves the registry down), `bobcat-resources →
vant, bobcat-core`. `vant` sits below `terminus`, so nothing in it may name a
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

Synchronous on every target: natively a directory; in the browser (PR4) an
OPFS directory through `createSyncAccessHandle`, which only a Worker has and
which is why the Store lives on the Render Worker. The Cache API itself is
asynchronous and is not a `Store`; a browser host that prefers it fills a
memory-only `vant` tier from it.

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
    type Store: Store;      // `NoStore` for a host with no persistent tier
    type Decoder: ImageDecoder;
    /// Taken apart once, when the resource system is built: the byte tier
    /// owns its Store.
    fn into_parts(self) -> (Self::Fetch, Option<Self::Store>, Self::Decoder);
    /// The budget for the byte tier over `Store`; the tier clamps it to the
    /// Store's quota. `u64::MAX` means "the quota, or unbounded".
    fn byte_budget(&self) -> u64 { u64::MAX }
}
```

The resource system (`Resources::with_platform`) reads `byte_budget`, takes
the parts, and opens a `ByteCache` over the Store unless the `Fetch` owns its
HTTP cache.

`bobcat-resources` ships `NativePlatform` (`NativeFetch`, `FsStore`,
`NativeDecoder`) and, on wasm32, `BrowserPlatform` (`BrowserFetch`, `NoStore`,
`BrowserDecoder`). An embedder with its own HTTP stack or storage implements
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
- (PR2) `vant::artifact::ArtifactCache<S>` — see "Code cache".

## PR1 (this PR): `vant` exists, embedders provide the platform

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

Not in PR1: moving `images.rs`/`sources.rs`/`registry.rs` orchestration into
`vant` (PR3), the code cache (PR2), the browser Store (PR4).

## PR2: QuickJS code cache

Lookup happens **at source delivery, in the fetcher**, not at compile time in
the bridge: a `require`/`loadModuleSync` must answer synchronously and the
browser's storage cannot, so the fetcher hashes the script text it is about to
deliver, consults the artifact cache and delivers `(text, Option<bytecode>)`
together. `LoadedSource::Module` gains `bytecode: Option<Bytes>`. The bridge's
three compile points (`qjs_compile_module_source`, `qjs_eval`, the `require`
wrapper) take optional bytecode and read it with `JS_ReadObject` instead of
compiling; on a miss they compile, `JS_WriteObject` the result and hand the
bytes back through a compile hook so the fetcher can store them. Serialising
the just-compiled object is cheap; nothing recompiles in the background.

Entry layout, verified **before** `JS_ReadObject` (which trusts its input):

```
magic | format version | BC_VERSION | engine build id (quickjs commit, bridge
version, strip flags, cfg) | 128-bit source hash (SipHash-1-3, as the
synthetic-source hash) | hash of module name/URL and require parameter list |
bytecode length | crc64 | bytecode
```

The module name is part of the key because the compile bakes it into stack
frames and `import.meta.url`. `.web.bundle` headers carry no content hash, so
the text itself is hashed.

Storage: `vant::artifact::ArtifactCache<S: Store>` — a process-wide memory
tier (`Arc`, shared by every group and view, bounded) in front of an optional
`Store` in its own namespace. Entries are content-addressed, so two processes
writing one key write identical bytes and either rename may win; there is no
shared index file (Lynx's single `meta.json` under a process-local mutex is
the pattern to avoid). Expiry: last-access TTL plus a byte budget plus a
namespace wipe when the engine build id changes. Without a Store the memory
tier alone serves, with identical keys, so adding storage later changes no
caller; this is also what gives `bobcat-server` cross-capture reuse.

## PR3: `terminus` owns image residency

The image pipeline's state (`ImageState`: entries, the bitmap tier, pin,
restore, refinement) moves into `vant::image::Residency<P>` and the painter
holds it, so the CPU tier releases a bitmap the GPU `Bitmaps` cache pinned
and restores only when both have dropped it. `FrameImages` stays as the test
seam. The registry and source cache move into `vant` generic over the sheet
type, and `bobcat-source` depends on `vant` instead of `bobcat-resources`.

## PR4: browser `Store`

`OpfsStore` in `bobcat-wasm` over `createSyncAccessHandle`, used for the byte
tier and the code cache; `BrowserFetch`/`BrowserDecoder` move there from
`bobcat-resources`.

## Platform constraints the design answers

| platform | storage | when it is taken away | consequence |
|---|---|---|---|
| iOS | `Library/Caches` | purged when disk is low (Apple's disk-usage guidance); app extensions share the directory | `PurgedWhenIdle`; index is rebuilt by `scan`; entries are self-describing |
| Android | `getCacheDir`, `getCacheQuotaBytes` | oldest `lastModified` first, any time | `EvictableAnytime`; `touch` moves mtime; budget bounded by the quota |
| macOS / Linux CLI | `~/Library/Caches/bobcat`, `$XDG_CACHE_HOME/bobcat` | user tools; concurrent CLI processes | atomic writes, no shared index |
| browser | OPFS / Cache API, per-origin quota | whole-origin eviction under pressure; Safari clears script-writable storage after 7 days without interaction | `EvictableAnytime`; persistence is best-effort, the browser HTTP cache carries HTTP freshness |
| bobcat-server | none | fresh per capture | `NoStore`; the process-wide artifact memory tier still shares bytecode across captures |
