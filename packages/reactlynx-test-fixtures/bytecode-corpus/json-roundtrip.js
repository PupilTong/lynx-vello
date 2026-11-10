// JSON.stringify with replacers, toJSON and indentation, and JSON.parse with a reviver.
var o = { a: 1, b: [true, null, 'x'], c: { d: undefined, e: function () {} }, date: { toJSON() { return 'D'; } } };
var s1 = JSON.stringify(o);
var s2 = JSON.stringify(o, ['a', 'b']);
var s3 = JSON.stringify(o, function (k, v) { return typeof v === 'number' ? v * 10 : v; });
var s4 = JSON.stringify([1, { x: 2 }], null, 2);
var parsed = JSON.parse('{"n":1,"arr":[1,2],"s":"\\u0041"}', function (k, v) { return k === 'n' ? v + 1 : v; });
globalThis.__result = JSON.stringify([s1, s2, s3, s4, parsed]);
