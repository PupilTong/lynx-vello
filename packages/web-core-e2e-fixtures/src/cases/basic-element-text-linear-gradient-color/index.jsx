// Copyright 2023 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
import { root } from '@lynx-js/react';
function App() {
  return (
    <view
      style={{
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      <text style='font-size: 30px; color: linear-gradient(green, yellow);'>
        gradient
      </text>
      <text style='font-size: 30px; '>
        <text style='color: linear-gradient(green, yellow);'>
          inline-gradient
        </text>
      </text>
      <text style='font-size: 30px; color: linear-gradient(green, yellow);'>
        <text>
          inline-inherit-gradient
        </text>
      </text>
    </view>
  );
}
root.render(<App></App>);
