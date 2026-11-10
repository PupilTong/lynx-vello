// new with spread arguments.
function P(a, b, c) { this.sum = a + b + c; this.n = arguments.length; }
var args = [1, 2];
var p = new P(...args, 3);
var d = new Date(...[2020, 0, 15]);
var arr = new Array(...[3]);
class K { constructor(...xs) { this.xs = xs; } }
globalThis.__result = JSON.stringify([p.sum, p.n, d.getFullYear(), d.getDate(), arr.length, new K(...'ab', 1).xs]);
