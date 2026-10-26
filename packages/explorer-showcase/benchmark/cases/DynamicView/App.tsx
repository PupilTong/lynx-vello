/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './style.css';

export function App() {
  const colors = {
    level1: [0, 1, 2, 3, 4, 5],
    level2: [
      0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
      21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    ],
    level3: [
      0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
      21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    ],
    level4: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  };

  return (
    <view className="root">
      {colors.level1.map(
        (index1) =>
          index1 % 2 == 1 && (
            <view
              className="outer"
              key={index1}
              style={{ backgroundColor: '#fa43e6' }}
            >
              {colors.level2.map(
                (index2) =>
                  index2 % 2 == 1 && (
                    <view
                      className="block1"
                      key={index2}
                      style={{ backgroundColor: '#cccccc' }}
                    >
                      {colors.level3.map(
                        (index3) =>
                          index3 % 2 == 1 && (
                            <view
                              className="block2"
                              key={index3}
                              style={{ backgroundColor: '#333333' }}
                            >
                              {colors.level4.map(
                                (index4) =>
                                  index4 % 2 == 1 && (
                                    <view
                                      className="block3"
                                      key={index4}
                                      style={{ backgroundColor: 'red' }}
                                    ></view>
                                  ),
                              )}
                            </view>
                          ),
                      )}
                    </view>
                  ),
              )}
            </view>
          ),
      )}
    </view>
  );
}
