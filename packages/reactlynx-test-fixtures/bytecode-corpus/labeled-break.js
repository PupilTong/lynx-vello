// Labeled break out of nested loops.
var found = null;
outer: for (var i = 0; i < 5; i++) {
  for (var j = 0; j < 5; j++) {
    if (i * j === 6) { found = [i, j]; break outer; }
  }
}
var log = [];
loop1: while (true) {
  loop2: do {
    log.push('inner');
    break loop1;
  } while (true);
}
globalThis.__result = JSON.stringify([found, log]);
