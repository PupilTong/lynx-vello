// A function with many locals and arguments, past the short-form opcode ranges.
function f(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9) {
  var v0 = a0, v1 = a1, v2 = a2, v3 = a3, v4 = a4, v5 = a5, v6 = a6, v7 = a7, v8 = a8, v9 = a9;
  var w0 = v0 + 1, w1 = v1 + 1, w2 = v2 + 1, w3 = v3 + 1, w4 = v4 + 1, w5 = v5 + 1, w6 = v6 + 1, w7 = v7 + 1, w8 = v8 + 1, w9 = v9 + 1;
  let x0 = w0 * 2, x1 = w1 * 2, x2 = w2 * 2, x3 = w3 * 2, x4 = w4 * 2, x5 = w5 * 2, x6 = w6 * 2, x7 = w7 * 2, x8 = w8 * 2, x9 = w9 * 2;
  var g = function () { return x0 + x1 + x2 + x3 + x4 + x5 + x6 + x7 + x8 + x9 + w9 + a9; };
  return [x0 + x9, g(), arguments.length];
}
globalThis.__result = JSON.stringify(f(0, 1, 2, 3, 4, 5, 6, 7, 8, 9));
