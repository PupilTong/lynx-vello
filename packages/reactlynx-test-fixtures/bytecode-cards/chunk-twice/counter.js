// Lepus chunk `counter`: counts its own runs on globalThis. No top-level declaration.
globalThis.__chunkTwiceCount = (globalThis.__chunkTwiceCount || 0) + 1;
globalThis.__chunkTwiceRuns.push('run' + globalThis.__chunkTwiceCount);
