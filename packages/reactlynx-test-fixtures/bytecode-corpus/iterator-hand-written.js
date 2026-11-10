// A hand-written iterable used by for-of, spread, Array.from and destructuring.
function countdown(n) {
  return {
    [Symbol.iterator]() {
      var cur = n;
      return { next() { return cur > 0 ? { value: cur--, done: false } : { value: 'done value', done: true }; } };
    },
  };
}
var out = [];
for (const v of countdown(3)) out.push(v);
var [first, ...others] = countdown(4);
globalThis.__result = JSON.stringify([out, [...countdown(2)], Array.from(countdown(2), x => x * 10), first, others, Math.max(...countdown(5))]);
