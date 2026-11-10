// Loops that count down with postfix and prefix decrement on locals.
function countdown(n) {
  var out = [];
  for (var i = n; i > 0; i--) out.push(i);
  var j = n;
  while (j) { j--; out.push('w' + j); }
  var k = 3;
  do { --k; } while (k > 0);
  out.push(k);
  let m = 2;
  m--;
  out.push(m);
  return out;
}
globalThis.__result = JSON.stringify(countdown(3));
