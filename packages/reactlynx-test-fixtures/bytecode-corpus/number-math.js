// Number formatting and parsing, and Math functions with exact results.
var out = [(255).toString(16), (255).toString(2), (3.14159).toFixed(2), (1234.5).toExponential(1), (0.000123).toPrecision(2), parseInt('42px'), parseInt('ff', 16), parseFloat('3.5e2abc'), Number('0b11'), Number.isInteger(5.0), Number.isSafeInteger(2 ** 53), Math.max(), Math.min(1, -2), Math.abs(-7), Math.floor(-1.5), Math.ceil(-1.5), Math.trunc(-1.7), Math.sign(-3), Math.hypot(3, 4), Math.sqrt(144), Math.cbrt(27), Math.clz32(1), Math.imul(0xffffffff, 5)];
globalThis.__result = JSON.stringify(out.map(String));
