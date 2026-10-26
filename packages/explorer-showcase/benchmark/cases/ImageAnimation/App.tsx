/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import {
  small_images,
  imageAnimationCount,
  imageAnimationColumn,
} from '../../data/images.ts';
import { getPaddedList } from '../../data/utils.ts';
const imageList = getPaddedList(small_images, imageAnimationCount);

export function App() {
  const renderContent = () => {
    const containerWidth = 370;
    const itemWidth = containerWidth / imageAnimationColumn;
    return (
      <view
        style={{
          height: '100%',
          width: `${containerWidth}px`,
          display: 'relative',
        }}
      >
        {imageList.map((avatarUrl: string, index: number) => {
          const left = (index % imageAnimationColumn) * itemWidth;
          const top = Math.floor(index / imageAnimationColumn) * itemWidth;
          const animations = [
            'rotateZ-ani 1s infinite',
            'scaleXY-ani 1s infinite',
            'opacity-ani 1s infinite',
          ];
          return (
            <image
              mode="aspectFill"
              src={avatarUrl}
              key={index}
              style={{
                width: `${itemWidth}px`,
                height: `${itemWidth}px`,
                left: `${left}px`,
                top: `${top}px`,
                position: 'absolute',
                animation: animations[index % animations.length],
              }}
            />
          );
        })}
      </view>
    );
  };
  return (
    <view style={{ width: '100%', height: '100%' }}>{renderContent()}</view>
  );
}
