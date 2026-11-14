// break, return and throw out of for-of call the iterator's return method.
var log = [];
function iter(name) {
  var i = 0;
  return {
    [Symbol.iterator]() { return this; },
    next() { return { value: i++, done: i > 5 }; },
    return() { log.push(name + ' return'); return { done: true }; },
  };
}
for (const v of iter('break')) { if (v === 1) break; }
function f() { for (const v of iter('return')) { if (v === 2) return v; } }
f();
try { for (const v of iter('throw')) { throw new Error(); } } catch (e) { log.push('caught'); }
outer: for (const a of [1, 2]) { for (const b of iter('labeled')) { continue outer; } }
for (const v of iter('complete')) {}
globalThis.__result = JSON.stringify(log);
