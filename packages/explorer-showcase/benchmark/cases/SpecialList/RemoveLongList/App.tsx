/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import { colors_1000 as colors } from '../../../data/colors.ts';
import { useEffect, useState } from '@lynx-js/react';

export function App() {
  const [colorList, setColorList] = useState(() =>
    colors.map((color, id) => ({ color, id })),
  );
  useEffect(() => {
    const timer = setTimeout(() => {
      setColorList((current) =>
        current.filter((_, index) => index < 4 || index >= 104),
      );
    }, 1000);
    return () => clearTimeout(timer);
  }, []);

  const renderContent = () => {
    return (
      <list
        style={{
          height: '100%',
          width: '100%',
        }}
        custom-list-name={'list-container'}
      >
        {colorList.map(({ color, id }, index) => {
          return (
            <list-item key={index} item-key={index.toString()}>
              <view
                style={{
                  backgroundColor: color,
                  height: '16.8px',
                }}
              >
                <text>{id}</text>
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
