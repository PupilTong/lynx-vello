// Compound assignment and ++/-- on captured variables.
function f() {
  var n = 1;
  let m = 10;
  var inc = () => { n += 2; m -= 1; n++; --m; return [n, m]; };
  inc();
  return inc();
}
function g() {
  const c = 0;
  return () => { try { c += 1; } catch (e) { return e.name; } };
}
globalThis.__result = JSON.stringify([f(), g()()]);
