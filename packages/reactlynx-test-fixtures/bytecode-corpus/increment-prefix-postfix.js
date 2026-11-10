// Prefix and postfix ++/-- on locals return the right values.
function f() {
  var i = 5, out = [];
  out.push(i++, i, ++i, i, i--, i, --i, i);
  var s = '7';
  out.push(s++, s);
  var u;
  u++;
  out.push(String(u));
  return out;
}
globalThis.__result = JSON.stringify(f());
