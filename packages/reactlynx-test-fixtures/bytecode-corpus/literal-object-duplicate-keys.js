// A later duplicate key wins and keeps the first key's position.
var o = { a: 1, b: 2, a: 3, get c() { return 'g'; }, c: 4 };
globalThis.__result = JSON.stringify([o, Object.keys(o)]);
