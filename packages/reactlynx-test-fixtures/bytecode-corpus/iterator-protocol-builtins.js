// Built-ins that consume iterables: Map, Set, Promise.all input, Object.fromEntries.
function* pairs() { yield ['a', 1]; yield ['b', 2]; }
var m = new Map(pairs());
var s = new Set('hello');
var o = Object.fromEntries(pairs());
var arr = Array.from({ length: 3 }, (_, i) => i * i);
var it = [1, 2][Symbol.iterator]();
globalThis.__result = JSON.stringify([m.get('b'), s.size, o, arr, typeof it.next, it[Symbol.iterator]() === it]);
