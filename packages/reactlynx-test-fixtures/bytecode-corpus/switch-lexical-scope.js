// The case clauses of a switch share one lexical scope.
function f(x) {
  switch (x) {
    case 0:
      let v = 'zero';
      return v;
    case 1:
      try { return v; } catch (e) { return e.name; }
    default:
      try { v = 'assigned'; return v; } catch (e) { return 'default ' + e.name; }
  }
}
var fns = [];
switch (1) { case 1: let captured = 'c'; fns.push(() => captured); }
globalThis.__result = JSON.stringify([f(0), f(1), f(2), fns[0]()]);
