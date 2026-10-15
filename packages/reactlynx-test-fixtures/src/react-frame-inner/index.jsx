// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-inner` case: the card a
// `react-frame` `<frame>` shows. The upstream card also prints its init data
// and `lynx.__globalProps`; a frame hands its card neither yet, so those two
// texts are dropped.
//
// A blue 100px view holding `frame:ready`. The component body logs
// `react-frame-inner-rendered`: a frame's card renders its first screen on
// the main thread alone, so the log comes from the frame's own realm.
import { root } from '@lynx-js/react';

function App() {
  console.log('react-frame-inner-rendered');
  return (
    <view style={{ width: '100px', height: '100px', backgroundColor: '#0000ff' }}>
      <text id='frame-ready'>frame:ready</text>
    </view>
  );
}

root.render(<App />);
