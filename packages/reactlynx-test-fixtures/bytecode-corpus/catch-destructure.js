// catch with a destructuring binding and catch-scoped shadowing.
var out = [];
try { throw { code: 42, info: ['a', 'b'] }; } catch ({ code, info: [first] }) { out.push(code, first); }
try { throw [1, 2]; } catch ([x, y]) { out.push(x + y); }
var e = 'outer';
try { throw 'inner'; } catch (e) { var e2 = e; }
out.push(e, e2);
globalThis.__result = JSON.stringify(out);
