// Numeric separators in decimal, hex, binary and fraction literals.
var out = [1_000_000, 0xFF_FF, 0b1010_0101, 1_0.5_0, 1e1_0];
globalThis.__result = JSON.stringify(out);
