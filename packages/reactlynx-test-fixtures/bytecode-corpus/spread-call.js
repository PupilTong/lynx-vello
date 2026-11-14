// Spread arguments in calls, mixed with ordinary arguments and iterables.
function f() { return Array.prototype.slice.call(arguments); }
var a = [1, 2];
var s = new Set([7, 8]);
var out = [f(...a), f(0, ...a, 3, ...a), f(...'hi'), f(...s), Math.max(...[3, 9, 4]), f(...[])];
globalThis.__result = JSON.stringify(out);
