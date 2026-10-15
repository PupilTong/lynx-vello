// Copyright 2023 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
//
// Copied from lynx-stack's web-core-e2e `api-getSharedData` case: a tap
// reads `color`, which `react-set-shared-data` stored from another view of
// the same group (web-core-e2e's `api-shared-context`).
import { useState, root, useEffect } from '@lynx-js/react';
function App() {
  const [color, setColor] = useState('orange');

  return (
    <view
      id='target'
      bindtap={() => {
        setColor(lynx.getSharedData('color'));
      }}
      style={{
        height: '100px',
        width: '100px',
        background: color,
      }}
    />
  );
}
root.render(<App />);
