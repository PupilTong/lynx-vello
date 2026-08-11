import { useCallback, useEffect, useRef, useState } from '@lynx-js/react';

import { createFlappy } from './lib/flappy.js';
import type { FlappyEngine, FlappyOptions } from './lib/flappy.js';

/** Returns the current bird height and a stable jump callback. */
export function useFlappy(
  options?: FlappyOptions,
): [number, () => void] {
  const [y, setY] = useState(0);
  const engineRef = useRef<FlappyEngine | null>(null);

  engineRef.current ??= createFlappy((newY) => {
    setY(newY);
  }, options);

  useEffect(() => {
    return () => {
      engineRef.current?.destroy();
    };
  }, []);

  const jump = useCallback(() => {
    'background-only';
    engineRef.current?.jump();
  }, []);

  return [y, jump];
}
