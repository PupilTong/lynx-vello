/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import { colors_1000 as colors } from '../../../data/colors.ts';
let color_10000: string[] = [];
for (let i = 0; i < 10; i++) {
  color_10000 = color_10000.concat(colors);
}
export function App() {
  const renderContent = () => {
    return (
      <list
        style={{
          height: '100%',
          width: '100%',
        }}
        custom-list-name={'list-container'}
      >
        {color_10000.map((color, index) => {
          return (
            <list-item item-key={index.toString()}>
              <view
                style={{
                  backgroundColor: color,
                  height: '16.8px',
                }}
              >
                <text>{index}</text>
              </view>
            </list-item>
          );
        })}
      </list>
    );
  };
  return (
    <view style={{ width: '100%', height: '100%' }}>{renderContent()}</view>
  );
}
