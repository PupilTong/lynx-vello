/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import { avatars } from '../../data/images.ts';
import { colors_1000 } from '../../data/colors.ts';
import './style.css';

const imageList = Object.values(avatars);
const itemWidth = 30;

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
        {colors_1000.map((color, index) => {
          const avatarUrl: string = imageList[index % imageList.length] ?? '';
          return (
            <list-item item-key={color + index}>
              <view
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  backgroundColor: color,
                  justifyContent: 'space-between',
                  padding: '2px',
                }}
              >
                <image
                  mode="aspectFill"
                  src={avatarUrl}
                  style={{
                    width: `${itemWidth}px`,
                    height: `${itemWidth}px`,
                  }}
                />
                <image
                  src={avatarUrl}
                  mode="aspectFill"
                  style={{
                    width: `${itemWidth}px`,
                    height: `${itemWidth}px`,
                    animation: 'rotateZ-ani 1s infinite',
                  }}
                />
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
