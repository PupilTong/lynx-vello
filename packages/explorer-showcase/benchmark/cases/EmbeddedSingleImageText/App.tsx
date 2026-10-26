/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import { posts, type BenchmarkCard } from '../../data/posts.ts';

export function App() {
  const renderCard1 = (card: BenchmarkCard) => {
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

  const renderList = () => {
    return (
      <view
        style={{
          height: '100%',
          width: '100%',
          display: 'linear',
          justifyContent: 'center',
          alignItems: 'center',
        }}
      >
        {renderCard1(posts[0]!.data)}
      </view>
    );
  };

  return renderList();
}
