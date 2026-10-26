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

  const renderCard2 = (card: BenchmarkCard) => {
    return (
      <view className="card">
        <view className="card2-container">
          <text className="card2-title">{card.content}</text>
          <image
            src={card.coverImage}
            accessibility-label="Benchmark pattern"
            className="card2-image"
            mode="aspectFill"
          />
          <text className="card2-bottom">{`${card.authorName} ${card.commentCount} updates ${card.creationDate}`}</text>
        </view>
      </view>
    );
  };

  const renderCard3 = (card: BenchmarkCard) => {
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

  const renderList = () => {
    return (
      <view
        style={{
          height: '100%',
          width: '100%',
        }}
      >
        {[
          renderCard1(posts[0]!.data),
          renderCard2(posts[1]!.data),
          renderCard3(posts[2]!.data),
          renderCard1(posts[3]!.data),
          renderCard2(posts[4]!.data),
          renderCard3(posts[5]!.data),
        ]}
      </view>
    );
  };

  return renderList();
}
