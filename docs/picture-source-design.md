# Picture sources: one path for images, animated images, video and other live content

Status: design, ruled 2026-11-14. Implementation proceeds in the steps of
[Delivery](#delivery); each step updates the section it implements and moves
its "planned" wording to the present tense.

This document is the specification the implementing changes are reviewed
against. Where it names a type that does not exist yet, the name is a
proposal and the contract is the requirement.

## Problem

Before step 2 the paint pipeline assumed a picture's pixels change only with
a commit:

- an image draw named its source by the URL the page wrote, and the painter
  resolved pixels through `terminus::link::FrameImages`, once per adopted
  frame (`Painter::adopt`);
- the compose key was the commit and the scroll generation, and a
  `filter`/`backdrop-filter` bake was keyed by commit; the one change of
  pixels within a commit, a wanted picture's restore arriving, cleared what
  the target held and the bakes rather than entering either key.

Animated images, video, a canvas and an off-screen web view all change
between commits, and none of them fits. Each could be added as its own
mechanism; this design adds one.

## Rulings this design implements

All by the user on 2026-11-14 unless dated otherwise.

1. Live content enters the engine as **texture input**: its frames are
   written into textures the engine draws through the normal scene. Platform
   views laid over or under the engine's surface are not built.
2. **Everything is asynchronous.** The painter never decodes and never
   blocks on a store. A picture that has neither pixels nor an upload draws
   nothing until its frame arrives; layout does not change. This replaces the
   2026-08-31 rule that a read of a loaded image must not miss.
3. **No decoder is compiled in.** Each platform decodes with its own API:
   ImageIO on macOS, the browser's `createImageBitmap` and WebCodecs
   `ImageDecoder`, gdk-pixbuf on Linux. GIF, APNG and animated WebP are all
   in scope; whether a format animates on a platform is that platform's
   decoder's answer.
4. The Lynx **`<image>` element is a `bobcat-core` component**, not a
   `terminus` feature. Its animation surface (`loop-count`, `autoplay`,
   `startplay` / `currentloopcomplete` / `finalloopcomplete`,
   `startAnimate` / `pauseAnimation` / `resumeAnimation` / `stopAnimation`)
   follows native Lynx: the behaviour Android and iOS share, and iOS's where
   they share none ([Animated images](#animated-images)).
5. **CSS images and a future HTML `<img>` follow W3C** and behave alike: they
   play automatically, for the loop count the file states, with no control
   surface. Native Lynx loops a CSS background forever; this is a deliberate
   deviation toward the web target.
6. Pictures with no control surface share **one playhead per URL**; each
   `<image>` element has its own.
7. Not built now, but the design must leave room for them without a second
   mechanism: canvas 2D, any canvas-family API visible to card JavaScript, an
   off-screen web view with input forwarding.
8. Video is the **smallest useful element**: `src`, `loop`, `muted`,
   `object-fit`; `play` and `pause`; `firstframe`, `ended`, `error`.

## Model

### Picture

A *picture* is anything a draw can sample. It has:

| Property | Meaning |
| --- | --- |
| key | `PictureKey::Url(Arc<str>)` — the URL the page wrote, shared by every draw of it — or `PictureKey::Id(PictureId)`, a process-unique id owned by one element. A replaced box presents either (`Document::set_picture`, which also gives the element its natural size) |
| generation | a counter that changes exactly when the picture's pixels do |
| descriptor | `vant::image::ImageDescriptor`: identity of the current bitmap, its size and layout, without pixels |
| opacity | whether any pixel may be non-opaque |
| next change | the instant the generation will next change by itself, when the picture knows it (an animated image does; a video does not), as a `ClockInstant` |

`FrameImages::describe(picture, hint)` answers all of these together as a
`FrameImage`; a store with no live pictures answers generation zero for
every one.

A static image is a picture whose generation never changes. Nothing about a
draw distinguishes the kinds: `object-fit`, sampling, the clip shape and the
scroll space of an image draw apply to every picture.

### Frame

A picture's content arrives as *frames*. A frame is one of:

- **CPU pixels** — `ImageData`, as bitmaps are today;
- **a platform frame** — a value of the embedder's own type (an
  `ImageBitmap` or `VideoFrame` in the browser, an `IOSurface`-backed buffer
  on macOS), named by an associated type so no `dyn` crosses the boundary.

`PictureFrame<P>` is the two; `FrameImages::PlatformFrame` is `P`. Until a
platform-frame writer exists the painter accepts only stores whose
`PlatformFrame` is `std::convert::Infallible`.

### Slot

Each `Id` picture has one *slot* (`PictureSlot`) holding its newest frame. A
producer on any thread replaces the slot's frame (`publish_pixels`) and wakes
the host's event loop when the slot says a wake is owed — when the frame it
replaced had been taken, or there was none; a frame replaced before it was
taken already owes one. Nothing is queued behind it. The store answers the
slot's description for `describe` and hands its frame over for
`FrameImages::take_frame`; the painter takes it at its next adoption, when the
generation differs from the one it drew. A `Url` picture has no slot: its
pixels are the store's, lent as before.

### Writer

A *writer* copies one frame into a texture the engine owns. It always runs on
the painter's thread, in `Painter::adopt`, before a swap-chain image is
acquired.

- Every bitmap is a texture of its own, drawn as a `vello_gpu` external
  texture: `vello_gpu` 0.4 removed its image atlas (`AtlasWriter` and
  `Renderer::upload_image` with it) and keeps an atlas for glyphs only. A
  `Url` picture's upload is keyed on its bitmap's descriptor; an `Id`
  picture's is keyed on its `PictureId`, one texture under one texture id,
  rewritten in place by a frame of the same size and replaced by a texture of
  the new size, under the same id, by one that is not
  (`BitmapLeases::write_picture` in `render::sparse`).
- The writer for CPU pixels is the engine's: it converts to premultiplied
  RGBA8, which is what a bound texture must hold, and queues the write. The
  writer for platform frames will be the embedder's, handed the device, the
  queue and the destination texture and its size; with no `AtlasWriter` in
  `vello_gpu` its exact shape is step 5's to decide.
- A frame is always copied. Sampling a producer's own texture is not built;
  the copy is where format conversion, orientation and premultiplication
  happen and what gives every picture one lifetime rule.

### What the painter does per adoption

A picture is *drawn* when the adopted commit draws it. The commit's walk has
already culled its draws to the encode window around each scrollport, so a
picture far off screen is not drawn; one inside the window but outside the
target is, which is what lets it scroll in with its frame already there.
Culling per composed frame instead would make the drawn set move with every
scroll frame.

1. For every picture the commit draws, ask the store for its state
   (descriptor, generation, next change), once per commit. At every later
   adoption ask again only for those that can change without a commit: a
   wanted picture, every `Id` picture, and one whose store named its next
   change. A commit whose pictures are all static URL images is asked
   nothing more. This never blocks.
2. For each `Id` picture whose generation differs from the one written into
   its texture, take the slot's frame and write it. If there is no frame and
   no texture, draw nothing for it and tell the store it is wanted.
3. Report the drawn set (`retain`, once per commit) and the uploads held
   (`release`), so the store can drop pixels and producers of pictures that
   are not drawn can stop producing.
4. Compose. The compose key is the commit, the scroll generation and a
   revision the painter moves whenever what a draw resolves to changes — a
   new frame taken, a wanted picture arriving — which is the generations of
   the drawn pictures folded into one number. A `filter` or
   `backdrop-filter` bake's key includes the bitmaps the image draws in its
   own range resolve to (`FilterGroup::pictures`, the draws of its range and
   of the backdrops it draws), so a new frame re-bakes only the bakes that
   draw it.
5. Frame demand: a taken frame, or a wanted picture arriving, owes one frame
   (`Painter::take_redraw`); a producer's wake is what gets the host to adopt.
   A live picture does not count as animating, so it does not keep the
   display link running. A known next-change instant is reported as
   `Painter::next_picture_change` for the host to arm a timer at; the painter
   has no timer of its own (the engine's timers run on the main thread, for
   realm timers and `x-swiper` autoplay), so arming one is the host's, and no
   host does yet: no store in the workspace answers a next change before
   step 4.

### Consequences accepted

- An evicted picture that scrolls back into view can be missing for a few
  frames. An upload of a drawn picture is leased and never trimmed, so this
  needs the picture to have left the screen first.
- A live picture under `filter: blur()` re-bakes on every new frame.
- A screenshot, and every golden, waits until no drawn picture is wanted and
  unanswered.

## Layers

| Layer | Owns |
| --- | --- |
| `terminus` | the picture key on a replaced box and in an image draw; per-picture upload and generation; compose and bake keys; frame demand; the writer for CPU pixels. Knows nothing of URLs beyond carrying them, of `<image>`, or of playback |
| `vant` | policy with no platform in it: residency, the animation timeline and playhead, the decoder and platform capability traits |
| `bobcat-resources`, `bobcat-wasm` | the platform's side: decoders, the platform-frame writer, the media player |
| `bobcat-core` | the elements: `image`, later `video`; attribute mapping, UI methods, events |

`terminus` keeps one primitive for a replaced element — the picture it
presents and its natural size, `Document::set_picture(node,
Option<(PictureKey, Option<(u32, u32)>)>)` — and resolves `Url` pictures, a
replaced box's and a CSS image's alike, against its image registry. It offers
that registry to its embedder as a URL image service with no element in it:
`Document::request_image(url) -> ImageStatus` asks for a URL once and answers
what is known of it (`Pending`, `Bitmap` or `Vector` with the natural size,
`Failed`), `Document::image_status` reads it without asking,
`Document::insert_image_document` files a document handed over as bytes under
its synthetic source, and `Document::apply_image_events` answers each source
a report settled with the status it settled at. A host URL is asked for once
and kept for the document's life; a document handed over as bytes is held by
the replaced boxes presenting it and forgotten with the last.

The `src` / `placeholder` roles, the choice between them and the
per-element `load` and `error` are `bobcat-core`'s, in
`main/tree/image_sources.rs` (`ImageSources`), which the `image` and `svg`
components and the runtime's report path share: it records the sources each
element names, presents the one the element shows at that picture's natural
size, and turns a settled `src` into an `ImageOutcome`.

The main thread holds picture keys and state only. It never holds pixels or a
frame.

## Animated images

One player serves every animated picture. It lives below the element layer,
so a CSS image needs no element code to animate.

- The **frame sequence** is shared per URL: decoded once, by the platform's
  decoder, a bounded window ahead of the playhead.
- A **playhead** holds the position, the loops completed and whether it is
  paused. A `Url` picture has one playhead for all its draws; an `Id`
  picture has its own.
- A playhead advances only while its picture is drawn. Time does not pass
  while it is not: it continues from the frame it stopped on.
- A frame that is not decoded when it is due is waited for. Frames are never
  skipped, so N loops produce exactly N loop events.
- A frame delay of 10 ms or less is 100 ms.
- Uploads are keyed by (animation, frame index), so playheads resting on the
  same frame share one.

A playhead's *policy* is the only difference between the two semantics:

| | `<image>` | CSS image, `<img>` |
| --- | --- | --- |
| loop count | `loop-count`: absent, 0 or negative is infinite; N plays N times; the file's own count is ignored | the file's own count |
| autoplay | `autoplay`, default on; off shows frame 0 | always |
| control | methods and events below | none |

`<image>` attributes are read when a `src` loads; a later change takes effect
at the next load. A `src` change resets position, loop count and paused
state.

| Method | Behaviour |
| --- | --- |
| `startAnimate` | restart at frame 0 with the loop count reset, from any state |
| `pauseAnimation` | stop advancing; the frame shown stays |
| `resumeAnimation` | continue from the paused position; after `stopAnimation` or the final loop, start at frame 0 |
| `stopAnimation` | stop advancing and move the playhead to frame 0; the frame shown stays |

Every method answers code 0, playing or not.

| Event | Fires |
| --- | --- |
| `startplay` | once when an animated `src` finishes loading with autoplay on, before `load`; never on resume or `startAnimate` |
| `currentloopcomplete` | at the end of every loop, the last included |
| `finalloopcomplete` | after the last `currentloopcomplete` of a finite count; never for an infinite count or a manual stop. The last frame stays shown |

All three carry an empty detail. An animated `placeholder` plays under the
CSS policy and fires nothing.

Where this differs from a platform: Android continues rather than restarts
on `startAnimate`, treats `stopAnimation` as a pause, advances by wall clock
while not drawn, and skips late frames. Those follow from its drawable
picking a frame by wall clock at each draw and are not reproduced.

## Platform capabilities

Associated types on `vant::Platform`, never `dyn`:

- **Decoder** — today's `ImageDecoder`, extended to answer an image's frame
  count, per-frame delays and stated loop count, and to decode frame *i*. Its
  output is CPU pixels or a platform frame.
- **Frame writer** — the platform-frame writer above. A platform with no
  platform frames names an uninhabited frame type and needs none.
- **Media** — opens a URL and returns a handle with `play`, `pause`,
  `set_loop`, `set_muted`, the events of ruling 8, and a slot the player
  fills.

Per platform:

| | Image decode | Animation frames | Video |
| --- | --- | --- | --- |
| macOS | ImageIO | ImageIO by frame index | AVFoundation, BGRA output as CPU frames at first |
| Browser | `createImageBitmap` in the Render Worker | WebCodecs `ImageDecoder`; first frame only where it is missing | `HTMLVideoElement` → `VideoFrame`, transferred to the Render Worker |
| Linux | gdk-pixbuf | first frame only, for now | none, for now |

The browser's main-thread `Image` decoder and its shared-memory mailbox are
removed: with nothing blocking, nothing needs them.

## Not built

- **Platform-view composition.** A cross-origin `<web-view>` in the browser,
  and a system web view on any platform, cannot be delivered as a texture and
  are therefore unsupported. An off-screen engine can be, through the frame
  writer. Recorded in `docs/tracking/deviations.md` when `<web-view>` is
  taken up.
- **Sampling a producer's texture without a copy.**
- **A video decoder of the engine's own**, DRM, HDR.
- **Canvas.** A canvas is a producer of CPU frames for an `Id` picture, drawn
  on the JavaScript thread; the slot and the CPU writer are everything it
  needs from this design. No canvas element, context or JavaScript API is
  added.
- **Input to a live picture.** An off-screen web view needs pointer, wheel,
  key and IME events routed to its producer in the picture's own coordinates.

## Delivery

One change per step, in order; each leaves the tree green.

1. **All asynchronous.** `FrameImages` has no blocking call: it is
   `describe`, `want`, `retain` and `release`, and the painter never decodes.
   A picture with neither pixels in the store nor an upload in the
   rasterizer draws nothing and is wanted, once per resolve; the store
   (`vant`'s `Residency::want`, run by `bobcat-resources` as a refinement
   job) restores it off the painter's thread and wakes the host, and the
   next adoption takes the pixels with no commit, clears what the target
   holds and forgets the `filter` bakes. A restore that fails leaves the
   picture with nothing to draw, and it is not retried. The document never
   learns of either. `Painter::awaits_pictures` is true while a drawn picture
   is wanted and unanswered; `Painter::capture` refuses such a frame, and
   the capturing hosts (the `bobcat` CLI's headless `screenshot`,
   `bobcat-server`) take their turns until it is false, under their frame
   deadline; `prime-radiant`'s `capture_document`, which has no turns to
   take, describes a wanted picture once more and refuses the capture if it
   is still missing. The blocking half of the browser mailbox (`Atomics.wait` and
   `Atomics.notify`) is gone; the asynchronous decode over the mailbox stays
   until step 5.
2. **Live pictures.** An image draw and a replaced box name a `PictureKey`
   (`Url` | `Id`); `Document::set_picture` puts an `Id` picture on an element
   with its natural size. A store's `FrameImage` carries a generation and a
   next change; an `Id` picture's CPU frames come from a `PictureSlot` and
   are taken with `FrameImages::take_frame`, which the painter writes into
   the picture's own texture in place. The compose key carries the revision
   of what the draws resolve to and each bake's key the bitmaps its range
   draws, so step 1's clearing of the target and the bakes is gone; a render
   that lost pixels it was lent still forgets the bakes, because a bake that
   failed to draw a bitmap can name the very bitmap it failed to draw. A
   change owes one frame; a known next change is reported
   (`Painter::next_picture_change`) for a host's timer, which no host arms
   yet. The drawn set is the commit's. `bobcat-resources` and
   `prime-radiant`'s `TestImages` answer URL pictures only, at generation
   zero. A test producer and goldens (`crates/terminus/tests/screenshots/live-picture/`):
   a picture that changes with no commit is recomposed; under `filter` it is
   re-baked; not drawn, it is not asked for; two draws share one texture; a
   change of size keeps the texture id; one frame is owed per change.
3. **`<image>` leaves `terminus`.** `Document::set_picture` takes a
   `PictureKey` of either kind with an optional natural size, and is the one
   replaced-box primitive: `set_image_source`, `set_image_document`,
   `image_source`, `ImageRole` and `ImageOutcome` are gone from `terminus`,
   and the paint walk draws the picture a box presents rather than choosing
   between two. The URL image service (`request_image`, `image_status`,
   `insert_image_document`, `apply_image_events` answering
   `(source, ImageStatus)` pairs) is what an embedder learns a URL's outcome
   and natural size from. `bobcat-core`'s `ImageSources` owns each `image`
   and `svg` element's sources, presents `src` while it is loaded, else a
   loaded `placeholder`, else whichever it names, at that picture's natural
   size, and produces `load` and `error` for `src` alone, at the write when
   the source had already settled and from the report otherwise; it drops an
   element's record when the element is freed (`CustomElement::freed`). The
   role and presented-choice tests moved with it. No behaviour change: every
   golden is unchanged.
4. **Animated images.** The player and its two policies in `vant`; the
   decoder's frame interface and its ImageIO implementation; the `<image>`
   attributes, methods and events; CSS images on shared playheads.
5. **Platform frames in the browser.** The frame-writer capability;
   `createImageBitmap` and `ImageDecoder` in the Render Worker; the
   main-thread decoder and mailbox removed.
6. **Minimal video.** The media capability, the `video` component, macOS and
   browser implementations.

## Open verification

- Current browser support for WebCodecs `ImageDecoder`.
- Whether `createImageBitmap` and the present browser path produce the same
  pixels (premultiplication, colour management). The wasm target has no
  rendering test in CI today, so step 5 needs one.
- What `vello_gpu` assumes about an external texture's colour space. Its
  alpha is settled: a texture bound with `TextureBindings::insert` is read as
  premultiplied RGBA, from mip level 0 of a non-array 2D view of a filterable
  float format, and its sides must fit in `u16`.
