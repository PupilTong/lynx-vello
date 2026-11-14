// Destructuring patterns in function parameters.
function f({ a, b = 2 }, [c, d = 4] = [3], ...[e, f2]) { return [a, b, c, d, e, f2]; }
var g = ({ x } = { x: 'default' }) => x;
function h([first, ...others]) { return others.length + first; }
var err;
try { f(); } catch (e) { err = e.name; }
globalThis.__result = JSON.stringify([f({ a: 1 }), f({ a: 1, b: 9 }, [7], 'e', 'f'), g(), g({ x: 'given' }), h('abc'), err]);
