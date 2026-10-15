// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-auto-height` case: a
// 300px-wide `<frame auto-height>` of `react-frame-inner` with no height,
// whose height is its card's content's.
//
// The page is red edge to edge. Its first child is a `display: flex` column
// with no padding, so the frame sits at the page's top-left and its height
// is its flex basis, its content's height: a column does not stretch its
// children's heights, where `react-frame`'s row would. The frame's green
// background, which upstream does not have, shows where the frame's box
// ends: right of the card's 100px blue view, and down to the bottom of its
// two 20px text rows, 140px from the top. Without `auto-height` the frame
// would be 0 tall and nothing of it would show. `src` is relative, so it
// resolves against the view's base URL, beside this page.
import { root } from '@lynx-js/react';

function App() {
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
        auto-height={true}
        src='./react-frame-inner.web.bundle'
        style={{ width: '300px', backgroundColor: '#00ff00' }}
      />
    </view>
  );
}

root.render(<App />);
