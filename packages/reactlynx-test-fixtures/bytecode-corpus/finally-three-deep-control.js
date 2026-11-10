// return, break and continue (labeled and unlabeled) that leave three nested
// try/finally blocks; every finally between the jump and its target runs, innermost
// first.
var log = [];
function ret(mode) {
  try {
    try {
      try {
        if (mode === 'return') return 'returned';
        if (mode === 'throw') throw new RangeError('x');
      } finally { log.push(mode + ' f1'); }
    } finally { log.push(mode + ' f2'); }
  } finally { log.push(mode + ' f3'); }
  return 'fell through';
}
var results = [ret('return'), ret('normal')];
try { ret('throw'); } catch (e) { results.push(e.name); }
outer: for (var i = 0; i < 3; i++) {
  try {
    for (var j = 0; j < 3; j++) {
      try {
        try {
          if (j === 0) continue;
          if (j === 1 && i === 0) continue outer;
          if (j === 1 && i === 1) break;
          if (i === 2) break outer;
        } finally { log.push('inner ' + i + j); }
      } finally { log.push('middle ' + i + j); }
    }
  } finally { log.push('outer ' + i); }
}
function returnFromLoop() {
  for (var k = 0; k < 5; k++) {
    try {
      try {
        try { if (k === 2) return 'k ' + k; } finally { log.push('rl1 ' + k); }
      } finally { log.push('rl2 ' + k); }
    } finally { log.push('rl3 ' + k); }
  }
}
results.push(returnFromLoop(), i, j);
globalThis.__result = JSON.stringify([results, log]);
