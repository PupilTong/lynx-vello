# Picture sources: one path for images, animated images, video and other live content

Status: design, ruled 2026-11-14. Implementation proceeds in the steps of
[Delivery](#delivery); each step updates the section it implements and moves
its "planned" wording to the present tense.

This document is the specification the implementing changes are reviewed
against. Where it names a type that does not exist yet, the name is a
proposal and the contract is the requirement.

## Problem

The paint pipeline assumes a picture's pixels change only with a commit:

- an image draw names its source by the URL the page wrote, and the painter
  resolves pixels through `terminus::link::FrameImages`, once per adopted
  frame (`Painter::adopt`);
- the compose key is the commit and the scroll generation, and a
  `filter`/`backdrop-filter` bake is keyed by commit;
- a restore after eviction decodes inside `FrameImages::read`, on the
  painter's thread, and the browser embedder keeps a shared-memory mailbox so
  a Worker can block for it.

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
| key | `Url(Arc<str>)` — the URL the page wrote, shared by every draw of it — or `Id(PictureId)`, a process-unique id owned by one element |
| generation | a counter that changes exactly when the picture's pixels do |
| descriptor | `vant::image::ImageDescriptor`: identity of the current bitmap, its size and layout, without pixels |
| opacity | whether any pixel may be non-opaque |
| next change | the instant the generation will next change by itself, when the picture knows it (an animated image does; a video does not) |

A static image is a picture whose generation never changes. Nothing about a
draw distinguishes the kinds: `object-fit`, sampling, the clip shape and the
scroll space of an image draw apply to every picture.

### Frame

A picture's content arrives as *frames*. A frame is one of:

- **CPU pixels** — `ImageData`, as bitmaps are today;
- **a platform frame** — a value of the embedder's own type (an
  `ImageBitmap` or `VideoFrame` in the browser, an `IOSurface`-backed buffer
  on macOS), named by an associated type so no `dyn` crosses the boundary.

### Slot

Each picture has one *slot* holding its newest frame. A producer on any
thread replaces the slot's frame and wakes the host's event loop; nothing is
queued behind it. The painter takes the frame when it next composes.

### Writer

A *writer* copies one frame into a rectangle of a texture the engine owns.
It always runs on the painter's thread, before a swap-chain image is
acquired.

- The engine picks the destination: an atlas page for a small picture, a
  texture of its own for a large one and for every picture whose generation
  changes. A changing picture is rewritten in place, under one texture id.
- The writer for CPU pixels is the engine's. The writer for platform frames
  is the embedder's, and has the shape of `vello_gpu`'s `AtlasWriter`
  (device, queue, encoder, destination texture, offset, size).
- A frame is always copied. Sampling a producer's own texture is not built;
  the copy is where format conversion, orientation and premultiplication
  happen and what gives every picture one lifetime rule.

### What the painter does per composed frame

1. For every picture the frame draws and can see, ask the store for its
   state (descriptor, generation, next change). This never blocks.
2. For each whose generation differs from the uploaded one, take the slot's
   frame and write it. If there is no frame and no upload, draw nothing for
   it and tell the store it is wanted.
3. Report the drawn set (today's `retain`) and the uploads held (today's
   `release`), so the store can drop pixels and producers of pictures that
   are not drawn can stop producing.
4. Compose. The compose key includes the generations of the pictures drawn;
   a `filter` or `backdrop-filter` bake's key includes the generations of the
   pictures inside it.
5. Frame demand: a taken frame, or a wake from a producer, owes one frame.
   A known next-change instant arms the engine's own timer, which then asks
   for one frame; a live picture does not keep the display link running.

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
presents and its natural size — and resolves CSS image URLs as `Url`
pictures. The `Source` / `Placeholder` roles, the choice between them at
paint time and the per-element load outcome leave `terminus` for the `image`
component.

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

1. **All asynchronous.** `FrameImages::read` loses its blocking contract and
   the painter stops decoding; a wanted picture is restored off the painter's
   thread and wakes the host. Captures wait for wanted pictures. The blocking
   half of the browser mailbox goes.
2. **Live pictures.** Picture keys (`Url` | `Id`), generations, slots for CPU
   frames, in-place rewrite of a picture's own texture, generations in the
   compose and bake keys, timer-driven frame demand, drawn-set feedback. A
   test producer and goldens: a picture that changes with no commit is
   recomposed; under `filter` it is re-baked; not drawn, it is not asked for.
3. **`<image>` leaves `terminus`.** Roles, the presented choice and load
   outcomes move to the `bobcat-core` component over the one replaced-box
   primitive. No behaviour change.
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
- What `vello_gpu` assumes about an external texture's alpha and colour
  space.
