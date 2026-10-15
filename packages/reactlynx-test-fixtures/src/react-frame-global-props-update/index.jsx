// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// `react-frame-data-update` for `global-props`, which upstream has no case
// for: a `<frame>` of `react-frame-inner` whose `global-props={{ message }}`
// comes from this card's state, `before` at first, and an
// `#update-frame-global-props` view whose tap sets it to `after`. The frame's
// card, which has rendered `global:before`, then re-renders with
// `global:after`: the frame hands the new global props to its card, whose
// background thread merges them into `lynx.__globalProps` and, in
// ReactLynx's default `reactive` global props mode, re-renders.
//
// Laid out as `react-frame-data-update` is: the page is red edge to edge, its
// `display: flex` column puts the 100px frame at the page's top-left, (0, 0)
// to (100, 100), and the yellow 100px `#update-frame-global-props` square
// right below it, (0, 100) to (100, 200), with an `update` text in its
// top-left corner, clear of its centre. The mount effect, which only the
// background thread runs, logs `react-frame-global-props-update-hydrated`
// once that thread rendered. `src` is relative, so it resolves against the
// view's base URL, beside this page.
import { root, useEffect, useState } from '@lynx-js/react';

function App() {
  const [message, setMessage] = useState('before');
  useEffect(() => {
    console.log('react-frame-global-props-update-hydrated');
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
        global-props={{ message }}
        style={{ width: '100px', height: '100px' }}
      />
      <view
        id='update-frame-global-props'
        bindtap={() => setMessage('after')}
        style={{ width: '100px', height: '100px', backgroundColor: '#ffff00' }}
      >
        <text>update</text>
      </view>
    </view>
  );
}

root.render(<App />);
