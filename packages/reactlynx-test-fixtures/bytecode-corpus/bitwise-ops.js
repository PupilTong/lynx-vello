// Bitwise operators and their 32-bit conversions.
var x = 0b1011, y = 0x0f;
var out = [x & y, x | 0x30, x ^ y, ~x, ~~3.7, ~~-3.7, 4294967295 | 0, 1.5e10 & 0xffff, -1 >>> 0, NaN | 0, '12' & 7];
globalThis.__result = JSON.stringify(out);
