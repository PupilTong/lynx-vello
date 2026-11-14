// Integer results that leave the 32-bit range and the int fast paths.
var max = 2147483647, min = -2147483648;
var out = [max + 1, min - 1, max * 2, min * -1, -min, max * max, (max + 1) | 0, 65536 * 65536, 46341 * 46341, min / -1, min % -1];
globalThis.__result = JSON.stringify(out.map(String));
