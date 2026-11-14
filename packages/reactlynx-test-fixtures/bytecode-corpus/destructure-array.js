// Array destructuring with holes, rest and iterables.
var [a, , b, ...rest] = [1, 2, 3, 4, 5];
let [x, y] = 'hi';
const [m, n] = new Set(['s1', 's2', 's3']);
var [p, q] = [1];
var [[i], [j, k]] = [[1], [2, 3]];
globalThis.__result = JSON.stringify([a, b, rest, x, y, m, n, p, typeof q, i, j, k]);
