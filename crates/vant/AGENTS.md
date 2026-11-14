# crates/vant

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/vant`. Paths in backticks are relative to the repository root.

The resource policy every embedder shares, below `terminus` (design:
`docs/resource-cache-design.md`). It names no DOM, style, runtime or platform
type and depends only on `bytes`, `http`, `url`, `serde_json`, `thiserror`,
`rustc-hash` and `vello_common` (for `peniko::ImageData`). What lives here:

- **The embedder capability traits**, modelled on the web platform's Fetch and
  Cache APIs: `Fetch` (a transport: `fetch_blocking` answering `None` where the
  host cannot block, async `fetch`, `owns_http_cache`, `map_file`), `Store`
  (synchronous persistent bytes with `Meta`, `estimate`, `touch`, `volatility`
  — `Durable`, `PurgedWhenIdle`, `EvictableAnytime` — and `map_body`),
  `AsyncStore` (the subset of `Store` the artifact tier uses, modelled on the
  web Cache API, every call but `volatility` a future that is `MaybeSend`),
  `ImageDecoder`, and `Platform`, the bundle an embedder hands over:
  `into_parts` answers `Parts { fetch, decoder, bytes, artifacts }`, the last
  two each an optional `Tier { store, budget }` — the HTTP byte tier's
  `Store`, and the derived-artifact tier's `ArtifactStore::Blocking(Store)` or
  `ArtifactStore::Async(AsyncStore)`, separate namespaces. Capabilities are
  traits with associated types, never `dyn`; `Threaded` is the cfg'd marker
  that requires `Send + Sync` natively and nothing on wasm32, and `MaybeSend`
  the one that requires `Send` natively and nothing on wasm32. `NoStore` is
  the `Store` and the `AsyncStore` of a host without one.
- **The tiers**: `memory::MemoryCache` (LRU by stamp under a byte budget with a
  pinned working set), `byte_cache::ByteCache<S: Store>` (the HTTP byte tier:
  RFC 9111 `plan` over a stored response, index rebuilt from `Store::scan` and
  treated as a hint, LRU by `last_used`, budget `min(configured, quota)`,
  `touch` rate-limited to once per 24 h per entry, an entry larger than the
  budget refused), and `http` (`StoredResponse`, freshness, revalidation
  headers, the JSON record a `Store` keeps beside a body).
- **The derived-artifact tier** (`artifact`, the code cache's storage; the
  crate never names QuickJS): `Artifacts`, the process-wide memory tier
  (`Artifacts::global()`, `Send + Sync` on every target, LRU under a byte
  budget) that an engine reads synchronously at the moment it would compile
  and inserts what it compiled into; and the persistence a resource system
  puts behind it — `preload(SourceDigest)` admits every stored variant of a
  source before its text is delivered, `flush` writes what was inserted
  since. Two drivers share the tier's IO-free decisions (one index type
  whose methods answer the IO to do and take its result):
  `ArtifactCache<S: Store>` runs the IO under the index lock, and
  `AsyncArtifactCache<A: AsyncStore>` awaits it with the lock released, its
  docs stating the rule each overlap of calls follows (two preloads of one
  source both read; a key whose put or delete is in flight is not written
  again; a read that fails deletes only the indexing it was issued for). A key is a `SourceDigest` (128-bit
  SipHash-1-3 of the text) and a `Variant` (what the derivation bakes in
  beyond the text) under an `EngineId`; a stored entry is `magic | format |
  engine | source | variant | length | crc64 | payload`, and `decode` is the
  only way a payload leaves a Store. Entries are content-addressed, so two
  processes writing one key write the same bytes and there is no shared index
  file; they expire 15 days after their last use, under a byte budget, and
  all at once when the `EngineId` changes.
- **The registry** (`registry`): contents the embedder already holds, served
  under a URL of its choosing. `Registry<S>` maps a normalized URL to a
  `Registered<S>` — bytes with an optional media type, or a stylesheet the
  host already parsed — and is generic over that stylesheet type `S`, since
  this crate names no style type. `Register<S>` (`register`,
  `register_style_sheet`) is what a registration is written through, and
  `RegisterInBackground<S>` adds running an install off the calling thread
  while every source request under the container URL is held; a source parser
  takes these and names no fetcher. `Registrar<S>` is `Register` over a
  registry alone, which is what a `ContainerInstaller<S>` — the hook that
  registers the sections of a container found in a plain fetch's bytes —
  writes through on whichever thread the load finished. The holds a
  background install places are not here: they wait on a tokio channel, so
  they are the orchestrator's.
- **Image residency** (`image::residency`): `Residency<W>`, the synchronous,
  IO-free state of every image source a host was asked for — loading with its
  waiters `W`, loaded, the parsed-document marker, failed — over the
  decoded-bitmap `MemoryCache`. A host asks what a request means
  (`Requested`: report now, join, or start a load at a bound), feeds every
  completion back (`loaded`, `parsed`, `failed`, `refined`, …) and is handed
  the waiters to tell; a frame's `read` answers `Read::Hit` with the
  `Refinement` to start when the bitmap is far from the drawn size,
  `Read::Restore` when it was evicted or released, or `Read::Unavailable`.
  `describe` answers without ever asking for a restore — `Described::Resident`,
  or `Described::Absent` with the `ImageDescriptor` (`vant::image`) of the
  bitmap last inserted for the source — and `release(sources)` drops bitmaps
  from the tier, pinned or not, for a caller whose rasterizer holds them; the
  entries stay loaded. Each view has
  a `ViewKey` and its own working set, and the tier pins **the union of
  every view's set** (`retain(view, frame)`, `release_view`), so one view's
  frame never unpins another's. It stores waiters and never calls them.
- **The vocabulary**: `CachePolicy`, `mime`, `preprocess`, `image_header`,
  `data_url`, `image` (`ImageSizeHint`, `Bitmap`, `DecodeError`,
  `target_size`, `wants_refinement`, `bounded`), and `error`
  (`ResourceError`, `ResourceErrorKind`, `ResourceErrorPhase`, `RetryAdvice`,
  `Failure`), which `bobcat-core` and `terminus` re-export at their old paths.

Not here: the load orchestration (the IO of `images.rs`, `sources.rs`, the
background install holds), which is `bobcat-resources`'s, the dispatch of
image reports, which is `terminus`'s `ImageResidency`, and every adapter — the native
ones are `bobcat-resources`'s and the browser's are `bobcat-wasm`'s.
