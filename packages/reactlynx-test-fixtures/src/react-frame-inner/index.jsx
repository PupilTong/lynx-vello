// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-inner` case: the card
// the `react-frame`, `react-frame-data` and `react-frame-auto-height`
// `<frame>`s show. Like upstream, it prints its init data's `label` and
// `lynx.__globalProps.message`, which a frame hands it from its `data` and
// `global-props`, under upstream's `#frame-data` and `#frame-global-props`.
//
// Two changes for pixel probes. `frame:ready` sits in a blue 100px view, and
// each of the other two texts is one 20px row below it, so the card's
// content is 140px tall whatever the fonts measure. A 100px frame shows the
// blue view alone and clips the rows.
//
// The component body logs `react-frame-inner-rendered`, then
// `react-frame-inner-data:<label>` and `react-frame-inner-global:<message>`
// (`undefined` for a frame that was given neither). A frame's card renders
// its first screen on the main thread alone, so the logs come from the
// frame's own realm, and they are what that first screen read.
import { root, useInitData } from '@lynx-js/react';

function App() {
  const initData = useInitData();
  const globalProps = lynx.__globalProps;
  console.log('react-frame-inner-rendered');
  console.log(`react-frame-inner-data:${initData.label}`);
  console.log(`react-frame-inner-global:${globalProps.message}`);
  return (
    <view>
      <view style={{ width: '100px', height: '100px', backgroundColor: '#0000ff' }}>
        <text id='frame-ready'>frame:ready</text>
      </view>
      <text id='frame-data' style={{ height: '20px' }}>data:{initData.label}</text>
      <text id='frame-global-props' style={{ height: '20px' }}>global:{globalProps.message}</text>
    </view>
  );
}

root.render(<App />);
