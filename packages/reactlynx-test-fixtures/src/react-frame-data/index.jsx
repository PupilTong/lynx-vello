// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-data` and
// `api-frame-global-props` cases, in one host: a `<frame>` of
// `react-frame-inner` given both `data={{ label: 'from-data' }}` and
// `global-props={{ message: 'from-global-props' }}`, which its card boots
// with as its init data and its `lynx.__globalProps`.
//
// Laid out as `react-frame` is: the page is red edge to edge, and its first
// child, a `display: flex` view with no padding, puts the 100px frame at
// the page's top-left, (0, 0) to (100, 100). `src` is relative, so it
// resolves against the view's base URL, beside this page.
import { root } from '@lynx-js/react';

function App() {
  return (
    <view style={{ display: 'flex', width: '100%', height: '100%', backgroundColor: '#ff0000' }}>
      <frame
        id='target'
        src='./react-frame-inner.web.bundle'
        data={{ label: 'from-data' }}
        global-props={{ message: 'from-global-props' }}
        style={{ width: '100px', height: '100px' }}
      />
    </view>
  );
}

root.render(<App />);
