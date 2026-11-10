// Integer and floating-point arithmetic on literals and variables.
var a = 7, b = 2;
var out = [a + b, a - b, a * b, a / b, a % b, -a % b, 7.5 % 2, 0.1 + 0.2, 2 ** 10, 1 / 3];
globalThis.__result = JSON.stringify(out);
