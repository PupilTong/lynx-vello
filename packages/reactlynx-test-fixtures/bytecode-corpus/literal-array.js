// Array literals with holes, trailing commas and nesting.
var a = [1, , 3];
var b = [, ,];
var c = [[1, [2, [3]]], 'x', { k: [4] }];
var d = [1, 2, 3,];
globalThis.__result = JSON.stringify([a.length, 1 in a, a, b.length, c, d.length, c[0][1][1][0], c[2].k[0]]);
