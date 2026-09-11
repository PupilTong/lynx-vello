# bobcat-server

`bobcat-server` is the `bobcat-cli` crate's `server`-gated HTTP embedder for
`bobcat-core`. Its screenshot protocol follows
[UI Judge at 83ad08f44eb5](https://github.com/PupilTong/lynx-stack/blob/83ad08f44eb5ed7c430e15fce6edc5121bc58495/packages/genui/ui-judge/src/server.rs).
`GET /health` returns `{"status":"ok"}` when the capture worker is ready.

Start it with:

```sh
LYNX_USE_PORT=8080 cargo run -p bobcat-cli --no-default-features --features server --bin bobcat-server
```

All screenshot routes accept `multipart/form-data`. Every parameter belongs
in a named part; JSON bodies, query parameters, duplicate parts, unknown fields,
and the former snake_case aliases are rejected. The former `POST /screenshot`
route is removed, as in UI Judge.

| Route | Required source part |
| --- | --- |
| `POST /screenshot/lynxml` | `source`: UTF-8 LynXML text or file |
| `POST /screenshot/template` | `url`: HTTP(S) compiled-template URL |
| `POST /screenshot/template/url` | `url`: HTTP(S) compiled-template URL |
| `POST /screenshot/zip/upload` | `file`: ZIP archive bytes |
| `POST /screenshot/zip/url` | `url`: HTTP(S) ZIP URL |

Every route requires `entry`: a safe relative path, such as
`pages/index.lynxml`, or its equivalent `zip:///pages/index.lynxml` URL.
The XML endpoint requires an entry ending in `.lynxml`; both template endpoints
require `.js`. ZIP endpoints select that entry from the supplied archive.

| Optional field | Contract |
| --- | --- |
| `width` | Positive integer, default `800` |
| `height` | Positive integer, default `600` |
| `initData` | JSON object encoded as a text part |
| `globalProps` | JSON object encoded as a text part; forbidden for `.lynxml` entries, even `{}` |
| `screenshotSettleMs` | Non-negative integer; only `/screenshot/template` and `/screenshot/lynxml`, default `16` |
| `timeoutMs` | Positive integer; only `/screenshot/template` and `/screenshot/lynxml`, default `60000` |

Both dimensions must be at most 8192, with at most 2,621,440 total pixels.
DPR is 1. ZIP routes and `/screenshot/template/url` use a fixed 500 ms settle
period and 60-second capture timeout; they reject the two timing fields.
Multipart values have a combined 10 MiB limit, with 64 KiB additional framing
allowed and a 10-second upload deadline. A `url` part is limited to 8 KiB;
remote downloads are limited to 10 MiB and 10 seconds.

Capture an XML page:

```sh
curl --request POST http://127.0.0.1:8080/screenshot/lynxml \
  --form-string 'entry=pages/index.lynxml' \
  --form-string 'width=375' \
  --form-string 'height=812' \
  --form-string 'screenshotSettleMs=16' \
  --form 'source=@/absolute/path/to/index.lynxml' \
  --output screenshot.bmp
```

Capture a template or ZIP:

```sh
curl --request POST http://127.0.0.1:8080/screenshot/template \
  --form-string 'entry=template.js' \
  --form-string 'url=https://cdn.example.com/card.web.bundle' \
  --output screenshot.bmp

curl --request POST http://127.0.0.1:8080/screenshot/zip/upload \
  --form-string 'entry=pages/index.lynxml' \
  --form 'file=@/absolute/path/to/page.zip;type=application/zip' \
  --output screenshot.bmp
```

A successful response contains raw BMP bytes, `Content-Type: image/bmp`,
`Cache-Control: no-store`, and the corresponding `Content-Length`. The BMP
matches UI Judge's runner: a 14-byte file header plus a 108-byte
`BITMAPV4HEADER`, 32-bit BGRA pixels, negative height (top-down rows),
`BI_BITFIELDS` with explicit RGBA masks, sRGB, and 2835 pixels per metre.
The encoder preserves the RGBA readback's alpha and does not composite it
again over white. The Bobcat renderer's existing white canvas remains its
rendering policy.

The request surface does not widen Bobcat's runtime support. Non-empty
`initData` and `globalProps` still return `422`: the server does not forward
them to the view yet. Empty supported objects are accepted. `task`, interaction
`steps`, and scoring fields no longer belong to the screenshot contract. This
embedder does not provide UI Judge's `/compare`.

Source loading uses the shared `bobcat-source` adapters for web bundles, raw
Lynx XML, source-based native bundles, and bounded ZIP archives. A template
entry named `.js` still needs a source format the adapter supports; arbitrary
JavaScript source is not a new input format. Binary page inputs require a
`root` module. Native QuickJS/Lepus bytecode remains unsupported. ZIP members
are registered in memory at their `zip:///` paths and retained for the view's
lifetime, without extraction to the filesystem. Shared ZIP decoder limits
remain 4096 entries and 128 MiB expanded data.

Axum accepts HTTP requests concurrently. An eight-item bounded queue feeds
one dedicated capture thread; each capture owns a fresh group, a view with its
resource registry, and an offscreen painter attached to that view. BMP encoding runs on Tokio's blocking pool.
A full/unavailable queue returns `503`; invalid multipart or ZIP input returns
`400`; page/render failures return `422`; capture/upload timeouts return `408`;
BMP encoding failures return `500`. Errors use `{"error":{"message":"…"}}`.

Remote URLs follow UI Judge's download policy: HTTP(S) only, no credentials,
no redirects, and public addresses only, with DNS results pinned for the
request. Invalid URLs return `400`, blocked addresses `403`, upstream failures
`502`, upstream timeouts `504`, and oversized bodies `413`.
The service listens on all IPv4 and IPv6 interfaces. It still requires trusted
page JavaScript: captures use fresh Bobcat groups on the existing owner
thread, rather than UI Judge's isolated child processes. Page subresources
use the existing Bobcat resource transport. `timeoutMs` cannot preempt
synchronous JavaScript, GPU driver work, or engine teardown. There is no auth,
TLS, or CORS layer.
