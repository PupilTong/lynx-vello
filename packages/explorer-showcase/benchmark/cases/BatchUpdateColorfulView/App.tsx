/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import './style.css';
import { useState, useEffect } from '@lynx-js/react';

const colors = {
  level1: ['77', '00', 'ff'],
  level2: [
    '00',
    '11',
    '22',
    '33',
    '44',
    '55',
    '66',
    '77',
    '88',
    '99',
    'aa',
    'bb',
    'cc',
    'dd',
    'ee',
    'ff',
  ],
  level3: [
    '00',
    '11',
    '22',
    '33',
    '44',
    '55',
    '66',
    '77',
    '88',
    '99',
    'aa',
    'bb',
    'cc',
    'dd',
    'ee',
    'ff',
  ],
  level4: ['00', '11', '22', '33', '44', '55', '66', '77'],
};

const colorsReversed = {
  level1: ['ff', '00', '77'],
  level2: [
    'ff',
    'ee',
    'dd',
    'cc',
    'bb',
    'aa',
    '99',
    '88',
    '77',
    '66',
    '55',
    '44',
    '33',
    '22',
    '11',
    '00',
  ],
  level3: [
    'ff',
    'ee',
    'dd',
    'cc',
    'bb',
    'aa',
    '99',
    '88',
    '77',
    '66',
    '55',
    '44',
    '33',
    '22',
    '11',
    '00',
  ],
  level4: ['77', '66', '55', '44', '33', '22', '11', '00'],
};

export function App() {
  const [level1Colors, setLevel1Colors] = useState(() =>
    colorsReversed.level1.map((c) => `#${c}${c}${c}`),
  );
  const [level2Colors, setLevel2Colors] = useState(() =>
    colorsReversed.level1.map((c1) =>
      colorsReversed.level2.map((c2) => `#${c1}${c2}${c2}`),
    ),
  );
  const [level3Colors, setLevel3Colors] = useState(() =>
    colorsReversed.level1.map((c1) =>
      colorsReversed.level2.map((c2) =>
        colorsReversed.level3.map((c3) => `#${c1}${c2}${c3}`),
      ),
    ),
  );
  const [level4Colors, setLevel4Colors] = useState(() =>
    colorsReversed.level1.map((_c1) =>
      colorsReversed.level2.map((c2) =>
        colorsReversed.level3.map((c3) =>
          colorsReversed.level4.map((c4) => `#${c2}${c3}${c4}`),
        ),
      ),
    ),
  );

  useEffect(() => {
    const timer = setTimeout(() => {
      // 更新 level1Colors
      setLevel1Colors(colors.level1.map((c) => `#${c}${c}${c}`));
      // 更新 level2Colors
      setLevel2Colors(
        colors.level1.map((c1) =>
          colors.level2.map((c2) => `#${c1}${c2}${c2}`),
        ),
      );
      // 更新 level3Colors
      setLevel3Colors(
        colors.level1.map((c1) =>
          colors.level2.map((c2) =>
            colors.level3.map((c3) => `#${c1}${c2}${c3}`),
          ),
        ),
      );
      // 更新 level4Colors
      setLevel4Colors(
        colors.level1.map((_) =>
          colors.level2.map((c2) =>
            colors.level3.map((c3) =>
              colors.level4.map((c4) => `#${c2}${c3}${c4}`),
            ),
          ),
        ),
      );
    }, 1000);
    return () => clearTimeout(timer);
  }, []);

  return (
    <view className="root">
      {colors.level1.map((_color1, i1) => (
        <view className="outer" style={{ backgroundColor: level1Colors[i1] }}>
          {colors.level2.map((_color2, i2) => (
            <view
              className="block1"
              style={{ backgroundColor: level2Colors[i1]?.[i2] }}
            >
              {colors.level3.map((_color3, i3) => (
                <view
                  className="block2"
                  style={{ backgroundColor: level3Colors[i1]?.[i2]?.[i3] }}
                >
                  {colors.level4.map((_color4, i4) => (
                    <view
                      className="block3"
                      style={{
                        backgroundColor: level4Colors[i1]?.[i2]?.[i3]?.[i4],
                      }}
                    ></view>
                  ))}
                </view>
              ))}
            </view>
          ))}
        </view>
      ))}
    </view>
  );
}
