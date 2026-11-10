// Mixed labeled break/continue across while, do-while, for-in and for-of.
var log = [];
var n = 0;
outer: do {
  n++;
  inner: for (var k in { a: 1, b: 2, c: 3 }) {
    for (const v of [1, 2]) {
      if (k === 'b') continue inner;
      if (n === 2 && k === 'c') break outer;
      if (v === 2) continue outer;
      log.push(n + k + v);
    }
  }
} while (n < 3);
globalThis.__result = JSON.stringify([log, n]);
