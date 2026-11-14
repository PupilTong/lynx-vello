// Number literal syntax: hex, octal, binary, exponent and fractions.
var out = [0x1F, 0o17, 0b101, 1e3, 2.5e-3, .5, 5., 0.1e1, 0X10, 0O7, 0B11, 9007199254740993, 1e21, 123456789012345680000];
globalThis.__result = JSON.stringify(out.map(String));
