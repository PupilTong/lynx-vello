// Strict equality and Object.is.
var o = {};
var out = [1 === 1, '1' === 1, o === o, {} === {}, NaN === NaN, 0 === -0, Object.is(0, -0), Object.is(NaN, NaN), null === undefined, 'a' !== 'a', undefined === void 0];
globalThis.__result = JSON.stringify(out);
