// Object destructuring with renaming, computed keys and rest.
var key = 'dyn';
var { a, b: renamed, [key]: computed, ...others } = { a: 1, b: 2, dyn: 3, c: 4, d: 5 };
let { length } = 'four';
const { toFixed } = 1;
var { missing } = {};
globalThis.__result = JSON.stringify([a, renamed, computed, others, length, typeof toFixed, typeof missing]);
