// A generator keeps locals and loop state across yields.
function* fib() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } }
var out = [];
for (const v of fib()) { if (v > 50) break; out.push(v); }
function* withArgs(a, b) { yield arguments.length; yield a + b; a = 10; yield arguments[0]; }
globalThis.__result = JSON.stringify([out, [...withArgs(1, 2)]]);
