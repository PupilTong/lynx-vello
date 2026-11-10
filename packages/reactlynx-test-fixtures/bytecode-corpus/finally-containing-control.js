// finally blocks that contain loops with break and continue, their own try/catch,
// nested try/finally and return statements that replace the pending completion.
var log = [];
function loopInFinally(mode) {
  try {
    if (mode === 'throw') throw new TypeError('pending');
    return 'try value';
  } finally {
    for (var i = 0; i < 4; i++) {
      if (i === 1) continue;
      if (i === 3) break;
      log.push(mode + ' loop ' + i);
    }
  }
}
function catchInFinally() {
  try {
    return 'kept';
  } finally {
    try { throw new SyntaxError('inside finally'); } catch (e) { log.push('caught ' + e.name); }
  }
}
function returnInFinally(mode) {
  try {
    if (mode === 'throw') throw new TypeError('discarded');
    return 'discarded value';
  } finally {
    if (mode !== 'keep') return 'finally value ' + mode;
  }
}
function nestedInFinally() {
  var n = 0;
  try {
    n = 1;
  } finally {
    try {
      n = 2;
      return n;
    } finally {
      log.push('inner finally sees ' + n);
      n = 3;
    }
  }
}
function finallyLoopReturn() {
  try {
    return 'outer';
  } finally {
    for (const v of ['a', 'b', 'c']) {
      try { if (v === 'b') return 'from loop ' + v; } finally { log.push('loop finally ' + v); }
    }
  }
}
var out = [loopInFinally('return'), catchInFinally(), returnInFinally('return'), returnInFinally('throw'), returnInFinally('keep'), nestedInFinally(), finallyLoopReturn()];
try { loopInFinally('throw'); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify([out, log]);
