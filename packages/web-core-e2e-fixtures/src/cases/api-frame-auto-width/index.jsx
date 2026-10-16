// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
import { root } from '@lynx-js/react';

function App() {
  return (
    <view
      style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start' }}
    >
      <frame
        id='target'
        auto-width={true}
        src='/dist/api-frame-inner.web.bundle'
        style={{ height: '120px', backgroundColor: 'yellow' }}
      />
    </view>
  );
}

root.render(<App />);
