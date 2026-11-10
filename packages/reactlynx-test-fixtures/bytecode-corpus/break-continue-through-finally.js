// break and continue inside try run the finally block.
var log = [];
for (var i = 0; i < 4; i++) {
  try {
    if (i === 1) continue;
    if (i === 3) break;
    log.push('body ' + i);
  } finally {
    log.push('finally ' + i);
  }
}
outer: for (var j = 0; j < 2; j++) {
  for (var k = 0; k < 2; k++) {
    try { if (k === 1) continue outer; } finally { log.push('f' + j + k); }
  }
}
lbl: try { break lbl; } finally { log.push('block finally'); }
globalThis.__result = JSON.stringify(log);
