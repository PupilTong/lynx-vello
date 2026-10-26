/**
 * Copyright 2025 The Lynx Authors. All rights reserved.
 * Licensed under the Apache License Version 2.0 that can be found in the
 * LICENSE file in the root directory of this source tree.
 */

import { useEffect, useState, type FC } from '@lynx-js/react';

import { countdowns } from '../../data/countdowns.ts';
interface CountdownProps {
  initialTime: number; // second
  frequency: number; // millisecond
  fontSize: number;
  color: string;
}

const Countdown: FC<CountdownProps> = ({
  initialTime,
  frequency,
  fontSize,
  color,
}) => {
  const [timeLeft, setTimeLeft] = useState(initialTime);

  useEffect(() => {
    const intervalId = setInterval(() => {
      setTimeLeft((prevTime) => {
        if (prevTime <= 0) {
          return initialTime;
        }
        return prevTime - 1;
      });
    }, frequency);

    return () => clearInterval(intervalId);
  }, [frequency, initialTime]);

  return (
    <text style={{ fontSize: `${fontSize}px`, color: color }}>{timeLeft}s</text>
  );
};

export const App: FC = () => {
  return (
    <view
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        width: '100vw',
        height: '100vh',
      }}
    >
      {countdowns.map((config, index) => (
        <Countdown
          key={index}
          initialTime={config.initialTime}
          frequency={config.frequency}
          fontSize={config.fontSize}
          color={config.color}
        />
      ))}
    </view>
  );
};
