// for-in over own and inherited enumerable string keys.
var proto = { inherited: 1 };
var o = Object.create(proto);
o.b = 1; o.a = 2; o[1] = 3;
Object.defineProperty(o, 'hidden', { value: 1, enumerable: false });
o[Symbol('s')] = 1;
var keys = [];
for (var k in o) keys.push(k);
var none = [];
for (var n in null) none.push(n);
for (var u in undefined) none.push(u);
for (var p in 'ab') keys.push('str' + p);
globalThis.__result = JSON.stringify([keys, none]);
