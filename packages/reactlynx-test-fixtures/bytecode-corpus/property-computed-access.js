// Computed member access converts the key with ToPropertyKey.
var o = {};
var key = { toString: function () { return 'k'; } };
o[key] = 1;
o[true] = 2;
o[null] = 3;
o[undefined] = 4;
o[[1, 2]] = 5;
var sym = Symbol('s');
o[sym] = 6;
globalThis.__result = JSON.stringify([o.k, o['true'], o['null'], o['undefined'], o['1,2'], o[sym], Object.keys(o)]);
