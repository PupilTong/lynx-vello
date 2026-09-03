# bobcat-server

`bobcat-server` is a native HTTP embedder for `bobcat-core`. It follows UI
Judge's screenshot wire contract: `GET /health` reports readiness and
`POST /screenshot` returns a raw 800×600, DPR-1 JPEG (quality 90) after
compositing Bobcat's RGBA readback over white.

Start it with:

```sh
LYNX_USE_PORT=8080 cargo run -p bobcat-server --bin bobcat-server
```

Capture a web bundle:

```sh
curl --request POST http://127.0.0.1:8080/screenshot \
  --header 'content-type: application/json' \
  --data '{
    "url": "file:///absolute/path/to/card.web.bundle",
    "task": "Capture the rendered page",
    "screenshotSettleMs": 16,
    "timeoutMs": 60000
  }' \
  --output screenshot.jpg
```

`url` accepts `file://`, `http://`, and `https://`. The current source loader
supports the web-target binary format and the existing raw Lynx XML source
envelope. Native `.lynx.bundle` bytecode is a different format and returns an
explicit `422` error; native-template support is deferred.

The request shape accepts UI Judge's camelCase fields and snake_case aliases.
Scoring-only fields are ignored. Non-empty `initialData`, `globalProps`, and
interaction `steps` return `422`, because the current opaque Bobcat runtime
does not expose faithful injection or selector-automation seams. Empty objects
and blank steps are accepted.

The HTTP runtime accepts requests concurrently. Rendering is admitted through
an eight-item bounded queue and performed sequentially on one dedicated owner
thread, where the non-`Send` `LynxView` is created with an offscreen GPU target,
ticked through the requested settle period, captured, and destroyed.
The resulting RGBA frame is JPEG-encoded on Tokio's blocking pool, allowing the
owner thread to begin the next GPU job. A full or unavailable queue returns
`503`; page/input/render failures return `422`; JPEG encoding failures return
`500`.

The service mirrors UI Judge by listening on all IPv4 and IPv6 interfaces and
by allowing local-file and network URLs. Run it only in a trusted environment;
it has no authentication, TLS, CORS, filesystem sandbox, or SSRF protection.
Page JavaScript must be trusted too: `timeoutMs` bounds asynchronous waits but
cannot preempt synchronous QuickJS execution, GPU driver work, or synchronous
engine teardown that has already begun.
