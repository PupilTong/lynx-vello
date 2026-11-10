// break out of labeled blocks through one and more finally blocks, including a
// finally that overrides the break with its own break or return.
var log = [];
outer: {
  try {
    inner: {
      try {
        log.push('inner try');
        break outer;
      } finally { log.push('inner finally'); }
    }
    log.push('skipped');
  } finally { log.push('outer finally'); }
  log.push('skipped too');
}
function f(mode) {
  var r = [];
  block: {
    try {
      try {
        if (mode === 'break') break block;
        if (mode === 'return') return 'returned';
        r.push('no jump');
      } finally {
        r.push('f1');
        if (mode === 'override') break block;
      }
      r.push('after f1');
    } finally { r.push('f2'); }
    r.push('end of block');
  }
  return r;
}
function overrideReturn() {
  b: {
    try { return 'try'; } finally { break b; }
  }
  return 'break in finally replaced the return';
}
log.push(f('break'), f('return'), f('none'), f('override'), overrideReturn());
loop: for (var i = 0; i < 3; i++) {
  blk: {
    try { if (i === 1) break blk; if (i === 2) break loop; } finally { log.push('iter ' + i); }
    log.push('body ' + i);
  }
}
globalThis.__result = JSON.stringify(log);
