/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import { posts, type BenchmarkCard } from '../../../data/posts.ts';
import { useState } from '@lynx-js/react';

export function App() {
  const [index, setIndex] = useState(2);

  const renderCard = (card: BenchmarkCard) => {
    const images = [card.coverImage, card.coverImage, card.coverImage];
    return (
      <view className="card">
        <view className="card2-container">
          <text className="card2-title">{card.content}</text>
          <view className="card3-images">
            {images.map((image, index) => {
              return (
                <image
                  key={index}
                  src={image}
                  accessibility-label="Benchmark pattern"
                  className="card3-image"
                  mode="aspectFill"
                />
              );
            })}
          </view>
          <text className="card2-bottom">{`${card.shareCount} passes ${card.creationDate}`}</text>
        </view>
      </view>
    );
  };

  return (
    <view bindtap={() => setIndex((current) => (current + 1) % posts.length)}>
      {renderCard(posts[index]!.data)}
    </view>
  );
}
