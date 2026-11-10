// Array methods that take callbacks, with index and this arguments.
var a = [5, 1, 4, 2, 3];
var ctx = { factor: 10 };
var out = [
  a.map(function (x, i) { return x * this.factor + i; }, ctx), a.filter(x => x % 2), a.reduce((s, x) => s + x, 0),
  a.reduceRight((s, x) => s + x, ''), a.find(x => x > 3), a.findIndex(x => x > 3), a.findLast(x => x < 3), a.some(x => x > 4),
  a.every(x => x > 0), a.slice().sort((x, y) => x - y), a.flatMap(x => [x, x]).length, [[1, [2]], 3].flat(Infinity), a.includes(4), a.at(-1),
];
globalThis.__result = JSON.stringify(out);
