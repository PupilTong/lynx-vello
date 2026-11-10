// switch compares with strict equality and evaluates case expressions lazily.
var log = [];
function c(v) { log.push('case ' + v); return v; }
function f(x) {
  switch (x) {
    case c(1): return 'number one';
    case c('1'): return 'string one';
    case c(NaN): return 'nan';
    default: return 'none';
  }
}
var r = [f('1'), f(NaN), f(1)];
globalThis.__result = JSON.stringify([r, log]);
