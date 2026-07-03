# DOM/element model, event system & gesture recognizers

> Status: pending initial research pass — see [README.md](README.md).

Will cover: the element/node tree API (`NodesRef`, `SelectorQuery`), event
types (`tap`/`longpress`/`touchstart`/`touchmove`/`touchend`/`touchcancel`,
etc.), the `bind`/`catch`/`capture-bind`/`capture-catch` event model (a
Lynx-specific deviation from the standard DOM `addEventListener`
capture+bubble + `stopPropagation`/`preventDefault` model — this file must
describe exactly how it differs and what W3C-standard event dispatch achieves
equivalent behavior), and gesture recognizers (pan/fling/rotation/scale/long-press,
including arbitration between simultaneous gestures).

Scope note: this is the spec for `.claude/agents/lynx-js-runtime-bridge.md`.

Implementation-pattern reference (not a behavior spec):
`/Users/akiwah/repos/paws-libs/Paws`'s `engine/src/events/` (`dispatch.rs`,
`event.rs`, `listener.rs`) and `engine/src/hit_test/` show standard DOM-style
event dispatch and hit-testing over a custom Rust DOM with no browser
underneath.
