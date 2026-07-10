# Style architecture

The style layer has one standards-oriented core and one Lynx adapter:

```text
lynx-widget  ───────▶  stylo-dom  ───────▶  vendor/stylo
Lynx policy            DOM + CSS core       parser/cascade primitives
```

The previous standalone `lynx-style` crate has been removed. Its generic
stylesheet/matching/cascade implementation moved down into `stylo-dom`; its
Lynx-only device and unit behavior moved up into `lynx-widget`.

![Style architecture before and after](img/style-architecture-refactor.svg)

## Ownership boundaries

| Layer | Owns | Must not own |
| --- | --- | --- |
| `stylo-dom` | `Element<T>`, `Arena<T>`, stylo DOM traits, invalidation, inline parsing, `Stylist`, rule matching, cascade, media evaluation, computed values, the private `SharedRwLock` | Lynx tags/PAPI, `WidgetState`, Lynx unit metrics, touch-device policy |
| `lynx-widget` | `WidgetState`, `WidgetTree`, PAPI validation, `EngineMetrics`, touch-first `Device` construction, viewport-relative `rpx` integration | A second stylist, cascade implementation, stylesheet lock sharing |
| `vendor/stylo` | CSS grammar, selector/rule-tree/cascade primitives, the maintained Lynx CSS extension patch set | Runtime Widget/PAPI policy |

## Style lifecycle

1. `lynx_widget::StyleEngine::new(EngineMetrics)` constructs the touch-first
   stylo `Device`; its viewport is the `rpx` basis.
2. The adapter constructs `stylo_dom::StyleEngine`, which owns one `Stylist`,
   one base URL, and one private `SharedRwLock`.
3. `StyleEngine::new_widget_tree()` asks the core for an arena bound to that
   private style context. Neither `lynx-widget` nor callers receive the lock.
4. DOM mutations and inline-style parsing happen in `Arena<T>` and mark the
   affected nodes dirty.
5. Stylesheets, selector matching, rule-tree insertion, inheritance, and
   cascade run in `stylo_dom::StyleEngine::resolve`.
6. The Widget adapter exposes `resolve_widget` and applies Lynx viewport/device changes;
   future flush orchestration can walk dirty Widgets without duplicating the
   CSS algorithm.

## Invariants

- Build styled arenas through the engine that will resolve them. `Arena::new`
  and `WidgetTree::new` are for standalone DOM-only use.
- `SharedRwLock` is an implementation detail of `stylo-dom`; embedders do not
  construct, pass, or read it.
- Standard CSS behavior belongs in `stylo-dom`. Lynx-only extensions and
  environment policy belong in `lynx-widget` (or the maintained stylo fork
  when they are first-class CSS grammar/value extensions).
- Device mutations go through `stylo_dom::StyleEngine::update_device` or
  `set_viewport`, ensuring media-dependent cascade data is refreshed.
