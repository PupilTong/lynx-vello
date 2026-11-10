// Operator precedence and associativity.
var out = [
  1 + 2 * 3, (1 + 2) * 3, 10 - 4 - 3, 2 * 3 % 4, 100 / 10 / 5,
  2 ** 3 ** 2, 1 + +'2', 1 - -1, 3 > 2 > 1, 1 < 2 < 3,
];
globalThis.__result = JSON.stringify(out);
