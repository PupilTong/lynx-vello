// Array destructuring closes the iterator when it stops early.
var log = [];
function iterable(n) {
  return {
    [Symbol.iterator]() {
      var i = 0;
      return {
        next() { log.push('next'); return { value: i++, done: i > n }; },
        return() { log.push('return'); return {}; },
      };
    },
  };
}
var [a, b] = iterable(5);
var [c, ...rest] = iterable(3);
var [] = iterable(1);
globalThis.__result = JSON.stringify([a, b, c, rest, log]);
