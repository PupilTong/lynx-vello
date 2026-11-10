// Destructuring assignment to existing variables and to member targets a.b and a[b].
var x, y, o = {}, arr = [], key = 'k';
[x, y] = [1, 2];
[x, y] = [y, x];
({ a: o.first, b: o[key], c: arr[0], d: arr[arr.length] = 'default' } = { a: 'A', b: 'B', c: 'C' });
[o.list, ...o.rest] = [1, 2, 3];
var r = ({ x } = { x: 99 });
globalThis.__result = JSON.stringify([x, y, o, arr, r.x]);
