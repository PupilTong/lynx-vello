// Subclasses of Array, Error, Map and Promise.
class MyArray extends Array { sum() { return this.reduce((a, b) => a + b, 0); } }
var a = MyArray.from([1, 2, 3]);
var mapped = a.map(x => x * 2);
class MyMap extends Map { getOr(k, d) { return this.has(k) ? this.get(k) : d; } }
var m = new MyMap([['a', 1]]);
globalThis.__result = JSON.stringify([a.sum(), mapped instanceof MyArray, mapped.sum(), a.length, Array.isArray(a), m.getOr('a', 0), m.getOr('b', 'none')]);
