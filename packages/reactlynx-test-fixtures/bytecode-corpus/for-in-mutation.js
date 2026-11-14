// Properties deleted during for-in are not visited.
var o = { a: 1, b: 2, c: 3, d: 4 };
var seen = [];
for (var k in o) {
  seen.push(k);
  if (k === 'a') delete o.c;
}
var target = {};
for (target.key in { x: 1, y: 2 }) seen.push('member ' + target.key);
globalThis.__result = JSON.stringify(seen);
