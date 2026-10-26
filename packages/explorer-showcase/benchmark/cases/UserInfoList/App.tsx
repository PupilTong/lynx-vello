/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import { posts, type BenchmarkCard } from '../../data/posts.ts';

export function App() {
  const renderCard = (card: BenchmarkCard, index: number) => {
    let indexColor;
    if (index === 0) {
      indexColor = '#F7283D';
    } else if (index === 1) {
      indexColor = '#F76A4F';
    } else if (index === 2) {
      indexColor = '#F4CB5F';
    } else {
      indexColor = '#AEADB2';
    }
    return (
      <list-item item-key={card.id.toString()}>
        <view className="card">
          <view className="card-container">
            <view className="card-left-container">
              <text className="card-index" style={{ color: indexColor }}>
                {index + 1}
              </text>
              <image
                className="avatar"
                src={card.coverImage}
                accessibility-label="Benchmark pattern"
                mode="aspectFill"
              />
              <view className="user-container">
                <text className="user-name">{card.authorName}</text>
                <text className="user-subtitle">{`${card.likes} sample views`}</text>
              </view>
            </view>
            <text className="card-left-title">{`${card.commentCount} runs`}</text>
          </view>
        </view>
      </list-item>
    );
  };

  const renderList = () => {
    return (
      <list
        list-type="single"
        style={{
          height: '100%',
          width: '100%',
        }}
        custom-list-name={'list-container'}
      >
        {posts.map((post, index) => {
          return renderCard(post.data, index);
        })}
      </list>
    );
  };

  return renderList();
}
