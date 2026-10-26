/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import type { FC } from '@lynx-js/react';
import { avatars, covers } from '../../data/images.ts';

const images = [...Object.values(avatars), ...Object.values(covers)];
const columns = 5;

export const App: FC = () => {
  return (
    <view>
      <list
        style={{ width: '100vw', height: '100vh' }}
        list-type="flow"
        column-count={columns}
        custom-list-name={'list-container'}
      >
        {images.map((item, index) => (
          <list-item item-key={index.toString()} key={index}>
            <image
              src={item}
              style={{
                width: '100%',
                aspectRatio: '1/1',
                objectFit: 'cover',
                padding: '2px',
              }}
            />
          </list-item>
        ))}
      </list>
    </view>
  );
};
