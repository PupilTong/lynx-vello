// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Adapted from lynx-stack's web-core-e2e `api-frame-inner` case: the card
// the `react-frame`, `react-frame-data`, `react-frame-auto-height`,
// `react-frame-data-update` and `react-frame-global-props-update`
// `<frame>`s show. Like upstream, it prints its init data's `label` and
// `lynx.__globalProps.message`, which a frame hands it from its `data` and
// `global-props`, under upstream's `#frame-data` and `#frame-global-props`.
//
// Three changes for pixel probes and logs. `frame:ready` sits in a 100px
// view, and each of the other two texts is one 20px row below it, so the
// card's content is 140px tall whatever the fonts measure. A 100px frame
// shows the view alone and clips the rows. The view is blue, and green once
// `label` or `message` reads `after`, which is what the
// `react-frame-data-update` and `react-frame-global-props-update` hosts
// change them to after first screen.
//
// The component body logs `react-frame-inner-rendered`, then
// `react-frame-inner-data:<label>` and `react-frame-inner-global:<message>`
// (`undefined` for a frame that was given neither). The body runs once on
// each of the card's threads for the first screen — the frame realm's main
// thread renders it, and the card's own background thread renders it again
// to hydrate — and again on the background thread for every re-render. The
// mount effect, which only the background thread runs, logs
// `react-frame-inner-hydrated` once, after that thread's first render.
import { root, useEffect, useInitData } from '@lynx-js/react';

function App() {
  const initData = useInitData();
  const globalProps = lynx.__globalProps;
  console.log('react-frame-inner-rendered');
  console.log(`react-frame-inner-data:${initData.label}`);
  console.log(`react-frame-inner-global:${globalProps.message}`);
  useEffect(() => {
    console.log('react-frame-inner-hydrated');
  }, []);
  const after = initData.label === 'after' || globalProps.message === 'after';
  return (
    <view>
      <view
        style={{ width: '100px', height: '100px', backgroundColor: after ? '#00ff00' : '#0000ff' }}
      >
        <text id='frame-ready'>frame:ready</text>
      </view>
      <text id='frame-data' style={{ height: '20px' }}>data:{initData.label}</text>
      <text id='frame-global-props' style={{ height: '20px' }}>global:{globalProps.message}</text>
    </view>
  );
}

root.render(<App />);
