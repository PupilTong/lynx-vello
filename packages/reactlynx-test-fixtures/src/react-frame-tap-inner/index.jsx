// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// No upstream case: the card the `react-frame-tap` `<frame>` shows, two
// 100px views in a column that each take a tap in this card's own threads.
//
// - `#tap-me`, blue, has a `catchtap` that runs on this card's background
//   thread: it logs `react-frame-tap-inner-at:<detail.x>,<detail.y>`, which
//   are relative to the frame's border box, and counts the tap in state,
//   which turns the view green. A `catchtap` ends this card's walk alone:
//   the host's `bindtap` around the frame still runs.
// - `#tap-me-on-main-thread`, magenta, has a `main-thread:bindtap` worklet,
//   which runs in this card's main-thread realm: it logs
//   `react-frame-tap-inner-main-thread-at:<detail.x>,<detail.y>` and paints
//   its own view green through `setStyleProperty`, with no state and no
//   background thread.
//
// The component body logs `react-frame-tap-inner-count:<count>`. It runs
// once on each of the card's threads for the first screen — the frame
// realm's main thread renders it, and the card's own background thread
// renders it again to hydrate — and again on the background thread for
// every re-render. The mount effect, which only the background thread runs,
// logs `react-frame-tap-inner-hydrated` once, after that thread's first
// render.
import { root, useEffect, useState } from '@lynx-js/react';

function App() {
  const [count, setCount] = useState(0);
  console.log(`react-frame-tap-inner-count:${count}`);
  useEffect(() => {
    console.log('react-frame-tap-inner-hydrated');
  }, []);
  return (
    <view>
      <view
        id='tap-me'
        catchtap={(e) => {
          console.log('react-frame-tap-inner-at:' + Math.round(e.detail.x) + ',' + Math.round(e.detail.y));
          setCount(c => c + 1);
        }}
        style={{ width: '100px', height: '100px', backgroundColor: count === 0 ? '#0000ff' : '#00ff00' }}
      />
      <view
        id='tap-me-on-main-thread'
        main-thread:bindtap={(e) => {
          'main thread';
          console.log(
            'react-frame-tap-inner-main-thread-at:' + Math.round(e.detail.x) + ',' + Math.round(e.detail.y),
          );
          e.currentTarget.setStyleProperty('background-color', '#00ff00');
        }}
        style={{ width: '100px', height: '100px', backgroundColor: '#ff00ff' }}
      />
    </view>
  );
}

root.render(<App />);
