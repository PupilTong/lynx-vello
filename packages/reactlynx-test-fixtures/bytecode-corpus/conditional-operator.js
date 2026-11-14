// The conditional operator, nested and right-associative.
function grade(n) { return n > 90 ? 'A' : n > 80 ? 'B' : n > 70 ? 'C' : 'F'; }
var t = true, f = false;
var out = [grade(95), grade(85), grade(75), grade(10), t ? f ? 1 : 2 : 3, (t ? 'x' : 'y') + (f ? 'x' : 'y'), 0 ? 'a' : '' ? 'b' : 'c'];
globalThis.__result = JSON.stringify(out);
