// return from for-of inside try/finally: iterator return runs, then finally.
var log = [];
function* gen() { try { yield 1; yield 2; } finally { log.push('gen finally'); } }
function f() {
  try {
    for (const v of gen()) {
      try { if (v === 1) return 'returned ' + v; } finally { log.push('inner finally'); }
    }
  } finally {
    log.push('outer finally');
  }
  return 'not reached';
}
var r = f();
function g() {
  for (const v of [1, 2]) {
    try { return v; } finally { log.push('array finally'); }
  }
}
globalThis.__result = JSON.stringify([r, g(), log]);
