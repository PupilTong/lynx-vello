// BigInt literals and arithmetic.
var a = 123456789012345678901234567890n;
var b = 2n ** 64n;
globalThis.__result = JSON.stringify([String(a * 2n), String(b), typeof a, String(-7n / 2n), 1n < 2, 0n == 0]);
