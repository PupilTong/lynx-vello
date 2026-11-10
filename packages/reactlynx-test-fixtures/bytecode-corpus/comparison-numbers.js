// Relational comparisons between numbers, NaN and mixed types.
var out = [1 < 2, 2 <= 2, 3 > 4, 4 >= 4, NaN < 1, NaN >= NaN, 1 < '2', '10' < '9', 10 < '9', null >= 0, undefined < 1, [2] > 1, -0 < 0];
globalThis.__result = JSON.stringify(out);
