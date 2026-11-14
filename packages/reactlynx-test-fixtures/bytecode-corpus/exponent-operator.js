// The ** operator and **=.
var x = 3;
x **= 2;
var y = 2;
y **= -1;
var out = [2 ** 0, 2 ** -2, (-2) ** 3, 4 ** 0.5, x, y, 2 ** 3 ** 0, (2 ** 3) ** 2, String(NaN ** 0), String(1 ** Infinity)];
globalThis.__result = JSON.stringify(out);
