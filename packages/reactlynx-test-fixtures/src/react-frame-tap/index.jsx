// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// No upstream case: a tap inside a `<frame>` runs the frame's card's own
// handlers, and this card's handlers on the frame's ancestors too. The frame
// shows `react-frame-tap-inner`, whose views take a tap themselves — one
// through a `catchtap` on its own background thread, one through a
// `main-thread:bindtap` worklet in its own main-thread realm — and the
// `#frame-wrapper` view around the frame here has a `bindtap` that logs
// `react-frame-tap-host:<target id>:<detail.y>`. This card hears the tap
// retargeted to the frame, so the target id is the frame's, `target`, and
// `detail.y` is the page's; the card's `catchtap` ends the card's own walk
// and nothing here (web-core's nested `lynx-view`, where each card walks its
// own shadow root).
//
// Laid out for pixel probes. The page is red edge to edge. Its first child,
// a `display: flex` column with no padding, puts a yellow 100px spacer at
// the page's top-left, (0, 0) to (100, 100), and the wrapper right below it,
// whose one child is the frame, 100px wide and 200px tall, (0, 100) to
// (100, 300): one 100px square for each of the card's two views. The mount
// effect, which only the background thread runs, logs
// `react-frame-tap-hydrated` once that thread rendered, so a test knows the
// wrapper's tap has a handler to reach. `src` is relative, so it resolves
// against the view's base URL, beside this page.
import { root, useEffect } from '@lynx-js/react';

function App() {
  useEffect(() => {
    console.log('react-frame-tap-hydrated');
  }, []);

  return (
    <view
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        height: '100%',
        backgroundColor: '#ff0000',
      }}
    >
      <view style={{ width: '100px', height: '100px', backgroundColor: '#ffff00' }} />
      <view
        id='frame-wrapper'
        bindtap={(e) => console.log('react-frame-tap-host:' + e.target.id + ':' + Math.round(e.detail.y))}
      >
        <frame
          id='target'
          src='./react-frame-tap-inner.web.bundle'
          style={{ width: '100px', height: '200px' }}
        />
      </view>
    </view>
  );
}

root.render(<App />);
