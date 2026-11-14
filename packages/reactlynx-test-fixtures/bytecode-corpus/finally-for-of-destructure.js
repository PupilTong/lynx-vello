// for-of loops and destructuring inside try/finally: break, return and throw from the
// loop body close the iterator before the finally runs, and destructuring that
// throws midway closes its iterator too.
var log = [];
function tracked(name, values) {
  return {
    [Symbol.iterator]() {
      var i = 0;
      return {
        next() { return i < values.length ? { value: values[i++], done: false } : { value: undefined, done: true }; },
        return() { log.push(name + ' closed'); return {}; },
      };
    },
  };
}
function loop(mode) {
  try {
    for (const [k, v] of tracked(mode, [['a', 1], ['b', 2], ['c', 3]])) {
      try {
        if (k === 'b' && mode === 'break') break;
        if (k === 'b' && mode === 'return') return 'returned ' + v;
        if (k === 'b' && mode === 'throw') throw new RangeError(k);
      } finally { log.push(mode + ' inner ' + k); }
    }
    return 'completed';
  } finally { log.push(mode + ' outer'); }
}
function destructure() {
  try {
    var [a, b] = tracked('pair', [1, 2, 3]);
    var { x, y: [y0, ...yRest] } = { x: 'X', y: tracked('rest', [4, 5, 6]) };
    var [first, second = (() => { throw new TypeError('default'); })()] = tracked('throws', [1]);
    return 'unreachable';
  } catch (e) {
    return [a, b, x, y0, yRest, e.name];
  } finally {
    log.push('destructure finally');
  }
}
var out = [loop('break'), loop('return'), loop('none')];
try { loop('throw'); } catch (e) { out.push(e.name); }
out.push(destructure());
globalThis.__result = JSON.stringify([out, log]);
