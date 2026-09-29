# Snap settle: how a paged scroller lands after the finger lifts

Research (2026-09-29) behind the painter's glide (`crates/bobcat-core/src/paint/motion.rs`,
`paint/inertia.rs`): what each reference does between the release of a drag and the moment a
paged or snapping container rests, compared with this engine. **Ruled 2026-09-29: the engine keeps
its own implementation**; this file records the alternatives it was weighed against.

Scope: the rule that picks the destination, the curve that reaches it, and whether the release
velocity carries into the curve. Events are out of scope. Rubber band, fling and bounce back are
in [css-layout.md](css-layout.md) (`overscroll-behavior`) and
[style-assumptions.md](../style-assumptions.md) §25.

## lynx-ui: two platform-independent implementations

`lynx-ui` (https://github.com/lynx-family/lynx-ui, commit `271d63e031a385b8749211ac043d54484d173804`)
is the Lynx team's component library. Its `view-pager` package wraps the native `<viewpager>`
element and leaves the animation to the platform
([`packages/lynx-ui-view-pager/src/index.tsx`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-view-pager/src/index.tsx)).
Two other packages animate a snap themselves, in main-thread script, writing a `transform` from a
`requestAnimationFrame` loop.

### Swiper (paged container)

Sources:
[`hooks/useOffset.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-swiper/src/hooks/useOffset.ts)
(`calcPaging`, `handleVelocity`, `handleTouchEnd`, `easingTo`),
[`hooks/useAnimate.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-swiper/src/hooks/useAnimate.ts),
[`hooks/useVelocity.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-swiper/src/hooks/useVelocity.ts),
[`utils/index.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-swiper/src/utils/index.ts)
(`easeOut`), defaults in
[`Swiper/index.tsx`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-swiper/src/Swiper/index.tsx).

- **Release velocity**: the samples of the last 50 ms (one older sample kept so sparse
  `touchmove` delivery still yields two points), pruned to 500 ms at read; first-to-last
  displacement over elapsed time, in px/s.
- **Destination**: `|v| > 300` px/s turns one page in the direction of `v` (`handleVelocity`);
  otherwise the offset rounds to the nearest page, threshold 0.5 (`calcPaging`). One page per
  release at most. `swipeNext`/`swipePrev` use thresholds 0.95 / 0.05 so a programmatic turn
  never skips a page.
- **Curve**: a fixed duration, default 500 ms, with the cubic ease-out `1 - (1 - p)^3`, from
  the offset at release to the destination. The release velocity does not enter the curve: a fast
  flick and a slow release take the same 500 ms. Authors may pass `main-thread:easing` or a whole
  `main-thread:customAnimation`.
- **Interruption**: the next `touchstart` cancels the animation.
- **Edges**: past an end, `d / (d / w + 1) * 1.5` with `d` capped at `2w`, where `w` is the
  bounce item's width (`rubberEffect` in `useOffset.ts`). This is not the `useBounce` rubber band
  the engine took for `overscroll-behavior: contain-bounce`.

### Sheet (snap points of a bottom sheet)

Sources:
[`hooks/useSnapTouches.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-sheet/src/hooks/useSnapTouches.ts)
(`handleTouchEndMT`),
[`hooks/useSnap.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-sheet/src/hooks/useSnap.ts)
(`animateToMT`),
[`utils/index.ts`](https://github.com/lynx-family/lynx-ui/blob/271d63e031a385b8749211ac043d54484d173804/packages/lynx-ui-sheet/src/utils/index.ts)
(`findNearestSnap`, `rubberEffect`).

- **Destination**: with `|v| >= 200` px/s (`flingMinVelocity`), the stop position of a constant
  deceleration of 2000 px/s² (`v² / (2·2000)`) is projected and clamped, and the nearest snap
  point to the projection wins; otherwise the nearest to the current position.
- **Curve**: `@lynx-js/motion`'s spring, stiffness 200, damping 60, mass 1 (`snapAnimation`
  default). Critical damping for that stiffness is `2·sqrt(200) ≈ 28.3`, so the default is
  overdamped and never crosses the target. A duration + `ease` form is accepted instead.
- **Edges**: `(1 - 1 / (x·c / max + 1)) · max`, `c = 0.3` — the `useBounce` shape with a
  different coefficient.

## Native platforms and web-core

| Platform | Destination | Curve and duration | Velocity carried | Source |
|---|---|---|---|---|
| Android `<viewpager>` | androidx `ViewPager.determineTargetPage`: a fling of more than 25 dp at more than 400 dp/s turns one page; otherwise the page offset truncates at 0.4 / 0.6. One page per release | androidx quintic ease-out `(t-1)^5 + 1`; duration from velocity or distance, capped at 600 ms | Yes, into the duration | androidx `ViewPager` (recalled, not in a local checkout); Lynx wrapper `lynx/platform/android/lynx_xelement/lynx_xelement_viewpager/.../CustomViewPager.kt` |
| iOS `<viewpager>` | `UICollectionView` with `pagingEnabled = YES` (`lynx/platform/darwin/ios/lynx_xelement/viewpager/LynxUIViewPager.m:48`): UIKit paging, one page per release | UIKit's; the source comment says the scroll "should be finished in 300ms" (`LynxUIViewPager.m:799`) | Yes | as cited |
| Harmony `<viewpager>` | `NODE_SCROLL_ENABLE_PAGING` (`lynx/platform/harmony/lynx_xelement/viewpager/ui_viewpager.cc:271-272`): ArkUI paging | ArkUI's; a programmatic turn animates 250 ms on `ARKUI_CURVE_SMOOTH` (`ui_viewpager.cc:179-180`) | Unknown | as cited |
| web-core `x-viewpager-ng` | Browser `scroll-snap-type: x mandatory` with `scroll-snap-stop: always` on each page (`lynx-stack/packages/web-platform/web-elements/src/elements/XViewpagerNg/x-viewpager-ng.css`) | The browser's snap animation | Browser-defined | as cited |

## This engine

`crates/bobcat-core/src/paint/inertia.rs` (`end_drag`, `Glide`) and `paint/motion.rs` (`glide`,
`glide_velocity`); the snap rule itself is `crates/dom/src/scroll/snap.rs` (css-scroll-snap-1).

- **Destination**: the css-scroll-snap-1 choice over the position the fling would naturally end
  at (exponential decay, UIKit's 0.998 per ms), clamped to the range; `scroll-snap-stop: always`
  on the pages keeps a release to one page. There is no velocity threshold: a slow flick moves
  the natural end a little, and the page turns only when that end passes the half-way point.
- **Curve**: a critically damped spring toward the destination, stiffness 15 /s (the same as the
  bounce back, taken from `useBounce`'s `beta`), `x(t) = (C + (v + β·C)·t)·e^(−β·t)`. About 95 %
  of the distance in 300 ms, at rest (within one physical pixel) within about 600 ms for a page-
  sized distance.
- **Velocity carried**: yes, as the spring's initial velocity, limited to `β·|C|` toward the
  target so the curve never crosses it; a velocity pointing away starts from rest (the engine's
  own rule; `useBounce` only springs from rest).
- **Far targets**: a destination more than one scrollport away keeps the aimed fling instead of
  the spring.
- **Interruption**: a drag step on the chain, a wheel step, or a newer programmatic scroll
  request.

## Comparison

| Aspect | lynx-ui Swiper | lynx-ui Sheet | Android | iOS | Harmony | web-core | This engine |
|---|---|---|---|---|---|---|---|
| Turn decided by | velocity > 300 px/s, else round at 0.5 | projection at 2000 px/s² decel, nearest | 25 dp + 400 dp/s, else 0.4 / 0.6 | UIKit paging | ArkUI paging | browser snap | natural fling end, nearest snap, `stop: always` |
| At most one page | yes | n/a | yes | yes | yes | yes (`stop: always`) | yes (`stop: always`) |
| Curve | cubic ease-out | overdamped spring | quintic ease-out | UIKit | ArkUI smooth | browser | critically damped spring |
| Duration | 500 ms fixed | spring-determined | ≤ 600 ms | ≈ 300 ms | 250 ms (programmatic) | browser | ≈ 300 ms to 95 %, ≈ 600 ms to rest |
| Release velocity in the curve | no | no (only in the destination) | yes | yes | unknown | browser | yes, capped |
| Interrupted by | next touch | next touch | touch | touch | not checked | touch | drag step, wheel, new request |

Same as lynx-ui's Swiper: one page per release, the animation runs on the presenting side without
layout, and the landing time is of the same order.

Different from it: the turn is decided by the snap rule over the natural fling end rather than by
a velocity threshold, so a very slow flick that would turn a Swiper page may spring back here; the
curve is a spring that keeps the release velocity rather than a fixed-duration ease-out; the
edge stretch is `useBounce`'s rubber band rather than the Swiper's.

## Side note

`lynx-ui`'s `view-pager` sets `align-width={true}` on the native `<viewpager>`. On iOS that is a
generic `LynxUI` prop (`lynx/platform/darwin/ios/lynx/ui/LynxUI.m:3584`, read in
`LynxUIOwner.m:937`); its effect was not investigated and it is not in
[components.md](components.md)'s viewpager row.
