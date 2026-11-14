// try/catch/finally inside a loop body, with exceptions on some iterations.
var log = [];
for (var i = 0; i < 5; i++) {
  try {
    if (i % 2) throw i;
    log.push('ok ' + i);
  } catch (e) {
    log.push('err ' + e);
    if (e === 3) continue;
  } finally {
    log.push('fin ' + i);
  }
  log.push('end ' + i);
}
globalThis.__result = JSON.stringify(log);
