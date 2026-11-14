// Nested try/finally blocks run innermost first, for return, throw and normal exit.
var log = [];
function f(mode) {
  try {
    try {
      try {
        if (mode === 'return') return 'r';
        if (mode === 'throw') throw 't';
      } finally { log.push(mode + ' f1'); }
    } finally { log.push(mode + ' f2'); }
  } finally { log.push(mode + ' f3'); }
  return 'normal';
}
var res = [f('return'), f('normal')];
try { f('throw'); } catch (e) { res.push('caught ' + e); }
globalThis.__result = JSON.stringify([res, log]);
