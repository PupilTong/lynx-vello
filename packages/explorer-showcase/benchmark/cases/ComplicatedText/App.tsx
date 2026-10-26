/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import type { FC } from '@lynx-js/react';

import { paragraphs } from '../../data/texts.ts';

export const App: FC = () => {
  return (
    <view
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        flexDirection: 'row',
        justifyContent: 'flex-start',
        alignItems: 'flex-start',
        justifyItems: 'flex-start',
        alignContent: 'flex-start',
        width: '100vw',
        height: '100vh',
      }}
    >
      {paragraphs.map((config, index) => (
        <text
          key={index}
          style={{
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            width: `${config.style.width}px`,
            height: `${config.style.height}px`,
            fontSize: `${config.style.fontSize}px`,
            color: config.style.color,
            fontStyle: config.style.fontStyle,
            fontWeight: `${config.style.fontWeight}`,
            textDecoration: config.style.textDecoration,
          }}
        >
          {config.text}
        </text>
      ))}
    </view>
  );
};
