// Symbol.unscopables hides properties from with.
var keys = 'outer keys';
var arr = [1, 2];
var out = [];
with (arr) { out.push(typeof keys, length); }
var o = { hidden: 'from object', shown: 'from object', [Symbol.unscopables]: { hidden: true } };
var hidden = 'outer hidden';
with (o) { out.push(hidden, shown); }
globalThis.__result = JSON.stringify(out);
