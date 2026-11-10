// return inside for-of and for await in an async generator closes the iterators
// the return leaves, innermost first.
var log = [];
function iterable(name) {
  return {
    [Symbol.iterator]() {
      var i = 0;
      return {
        next() { i++; return { value: name + i, done: i > 3 }; },
        return() { log.push(name + ' return'); return {}; },
      };
    },
  };
}
async function* fromSync() {
  for (const v of iterable('s')) { yield v; return 'early ' + v; }
}
async function* fromAsync() {
  var src = {
    [Symbol.asyncIterator]() {
      return {
        next() { return Promise.resolve({ value: 'a', done: false }); },
        return() { log.push('async return'); return Promise.resolve({ done: true }); },
      };
    },
  };
  for await (const v of src) {
    try { yield v; } finally { log.push('inner finally'); }
    return 'early ' + v;
  }
}
async function* nested() {
  for (const a of iterable('x')) {
    for (const b of iterable('y')) { return a + b; }
  }
}
async function main() {
  var out = [];
  for await (const v of fromSync()) out.push(v);
  var it = fromSync();
  out.push(await it.next(), await it.next(), await it.next());
  for await (const v of fromAsync()) out.push(v);
  out.push(await nested().next());
  return [out, log];
}
main().then(function (v) { globalThis.__result = JSON.stringify(v); });
