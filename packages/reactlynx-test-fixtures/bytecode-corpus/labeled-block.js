// break out of a labeled block and a labeled if statement.
var log = [];
block: {
  log.push('start');
  if (log.length) break block;
  log.push('skipped');
}
check: if (true) {
  log.push('in if');
  break check;
}
nested: { inner: { log.push('inner'); break nested; } log.push('never'); }
globalThis.__result = JSON.stringify(log);
