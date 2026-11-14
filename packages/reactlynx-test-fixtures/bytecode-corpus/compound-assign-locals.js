// Compound assignment operators on local variables.
function f() {
  var a = 10, s = 'x', b = 6;
  a += 5; a -= 3; a *= 2; a /= 4; a %= 4;
  s += 1; s += 'y';
  b <<= 2; b >>= 1; b >>>= 1; b |= 8; b &= 13; b ^= 5;
  var p = 2; p **= 5;
  let l = 1; l += l;
  return [a, s, b, p, l];
}
globalThis.__result = JSON.stringify(f());
