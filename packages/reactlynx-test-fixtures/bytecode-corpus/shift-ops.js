// Shift operators, including counts outside 0..31.
var out = [1 << 31, 1 << 32, 1 << 33, -16 >> 2, -16 >>> 28, 5 >> -1, 0x80000000 >> 0, 0x80000000 >>> 0, 3 << '2', (2 ** 40) >> 1];
globalThis.__result = JSON.stringify(out);
