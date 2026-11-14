// NaN, Infinity and negative zero produced by arithmetic.
var zero = 0;
var negZero = -zero;
var out = [
  String(0 / 0), String(1 / zero), String(-1 / zero), Object.is(negZero, -0),
  Object.is(0 * -1, -0), Object.is(-0 + 0, 0), 1 / negZero, String(Infinity - Infinity),
  isNaN(undefined + 1), Math.round(-0.5) === 0, Object.is(Math.round(-0.4), -0),
];
globalThis.__result = JSON.stringify(out.map(String));
