// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-data-update` case: a
// `<frame>` of `react-frame-inner` whose `data={{ label }}` comes from this
// card's state, `before` at first, and an `#update-frame-data` view whose tap
// sets it to `after`. The frame's card, which has rendered `data:before`,
// then re-renders with `data:after`: this card's background thread re-renders
// the frame's `data`, the frame hands it to its card as an update, and that
// card's own background thread re-renders.
//
// Laid out for pixel probes. The page is red edge to edge. Its first child,
// a `display: flex` column with no padding, puts the 100px frame at the
// page's top-left, (0, 0) to (100, 100), and the yellow 100px
// `#update-frame-data` square right below it, (0, 100) to (100, 200), with
// upstream's `update` text in its top-left corner, clear of its centre. The
// mount effect, which only the background thread runs, logs
// `react-frame-data-update-hydrated` once that thread rendered, so a test
// knows the tap has a handler to reach. `src` is relative, so it resolves
// against the view's base URL, beside this page.
import { root, useEffect, useState } from '@lynx-js/react';

function App() {
  const [label, setLabel] = useState('before');
  useEffect(() => {
    console.log('react-frame-data-update-hydrated');
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
      <frame
        id='target'
        src='./react-frame-inner.web.bundle'
        data={{ label }}
        style={{ width: '100px', height: '100px' }}
      />
      <view
        id='update-frame-data'
        bindtap={() => setLabel('after')}
        style={{ width: '100px', height: '100px', backgroundColor: '#ffff00' }}
      >
        <text>update</text>
      </view>
    </view>
  );
}

root.render(<App />);
