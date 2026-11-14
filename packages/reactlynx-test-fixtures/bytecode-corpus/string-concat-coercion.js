// + with strings, numbers, objects, arrays and null.
var o = { toString: function () { return 'O'; } };
var v = { valueOf: function () { return 10; }, toString: function () { return 'V'; } };
var out = ['a' + 1, 1 + 2 + 'a', 'a' + 1 + 2, [1, 2] + [3], {} + 'x', o + '!', v + 1, v + '', `${v}`, null + 'n', undefined + 's', true + 1, [] + []];
globalThis.__result = JSON.stringify(out);
