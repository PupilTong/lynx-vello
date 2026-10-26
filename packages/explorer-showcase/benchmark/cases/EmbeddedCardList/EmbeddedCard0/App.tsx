/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import { posts, type BenchmarkCard } from '../../../data/posts.ts';
import { useState } from '@lynx-js/react';

export function App() {
  const [index, setIndex] = useState(0);

  const renderCard = (card: BenchmarkCard) => {
    return (
      <view className="card">
        <view className="card1-container">
          <view className="card1-left-container">
            <text className="card1-title">{card.title}</text>
            <view className="card1-bottom">
              <text className="card1-search-red">Benchmark</text>
              <text className="card1-search">{`${card.likes} sample operations`}</text>
            </view>
          </view>
          <image
            src={card.coverImage}
            accessibility-label="Benchmark pattern"
            className="card1-image"
            mode="aspectFill"
          />
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
