// Method calls with spread keep the receiver.
var o = {
  base: 10,
  add(...xs) { return xs.reduce((a, b) => a + b, this.base); },
  nested: { v: 'n', get(...xs) { return this.v + xs.length; } },
};
var key = 'add';
var out = [o.add(...[1, 2]), o[key](...[3], 4), o.nested.get(...[1, 2, 3]), [1, 2, 3].concat(...[[4], [5]]), 'a-b-c'.split(...['-'])];
globalThis.__result = JSON.stringify(out);
