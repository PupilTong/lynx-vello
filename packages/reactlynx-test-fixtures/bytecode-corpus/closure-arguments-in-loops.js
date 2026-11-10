// Closures capture parameters and the arguments object from inside loops, while
// the loop modifies the parameters and the closures run after the loop ends.
function sloppy(a, b) {
  var fns = [];
  for (var i = 0; i < 3; i++) {
    if (i === 1) { a = 'changed a'; continue; }
    fns.push(function () { return [a, b, arguments.length, i]; });
    fns.push(() => [arguments[0], arguments.length]);
    if (i === 2) break;
  }
  b = 'changed b';
  return fns.map(function (f) { return f(); });
}
function strict(a, b) {
  'use strict';
  var fns = [];
  for (let i = 0; i < 2; i++) {
    a += i;
    fns.push(() => [a, arguments[0], arguments.length, i]);
  }
  return fns.map(f => f());
}
function rest(first, ...others) {
  var fns = [];
  for (const o of others) {
    if (o === 'skip') continue;
    fns.push(() => first + o + others.length);
  }
  return fns.map(f => f());
}
globalThis.__result = JSON.stringify([sloppy('A', 'B', 'C'), strict(1, 2, 3), rest('x', 'y', 'skip', 'z')]);
