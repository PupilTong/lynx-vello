/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './styles.css';
import { posts, type BenchmarkCard } from '../../data/posts.ts';

export function App() {
  const renderAuthorInfo = (card: BenchmarkCard) => {
    return (
      <view className="author-info">
        <image
          src={card.authorAvatar}
          accessibility-label="Explorer avatar"
          className="author-avatar"
          mode="aspectFill"
        />
        <view className="author-text-info">
          <text className="author-name">{card.authorName}</text>
        </view>
      </view>
    );
  };

  const HeartIcon = () => <text>❤️</text>;

  const renderCardStats = (card: BenchmarkCard) => {
    return (
      <view className="card-stats">
        <view className={`heart-icon ${card.liked ? 'liked' : ''}`}>
          <HeartIcon />
        </view>
        <text className="likes-count">{card.likes}</text>
      </view>
    );
  };

  const renderCardBottomInfo = (card: BenchmarkCard) => {
    return (
      <view className="card-bottom-info">
        {renderAuthorInfo(card)}
        {renderCardStats(card)}
      </view>
    );
  };

  const renderCardTags = (card: BenchmarkCard) => {
    return (
      <view className="tags-section">
        {card.tags.map((tag, index) => (
          <text key={index} className="tag">
            {tag}
          </text>
        ))}
      </view>
    );
  };

  const renderCardBody = (card: BenchmarkCard) => {
    return (
      <view className="card-body">
        <text className="card-title">{card.title}</text>
        {renderCardBottomInfo(card)}
        {renderCardTags(card)}
        <text className="card-content">{card.content}</text>
        <text className="creation-date">Fixture: {card.creationDate}</text>
      </view>
    );
  };

  const renderCard = (card: BenchmarkCard) => {
    return (
      <list-item item-key={card.id.toString()}>
        <view className="card">
          <image
            src={card.coverImage}
            accessibility-label="Benchmark pattern"
            className="header-image"
            mode="aspectFill"
          />
          {renderCardBody(card)}
        </view>
      </list-item>
    );
  };

  const renderWaterfall = () => {
    return (
      <list
        list-type="waterfall"
        column-count={2}
        style={{
          height: '100%',
          width: '100%',
        }}
        custom-list-name={'list-container'}
      >
        {posts.map((post) => {
          return renderCard(post.data);
        })}
      </list>
    );
  };

  return renderWaterfall();
}
