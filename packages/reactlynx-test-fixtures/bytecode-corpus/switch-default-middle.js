// A default clause in the middle is taken only when nothing matches, and falls through.
function f(x) {
  var log = [];
  switch (x) {
    case 'a': log.push('a');
    default: log.push('default');
    case 'b': log.push('b'); break;
    case 'c': log.push('c');
  }
  return log;
}
globalThis.__result = JSON.stringify([f('a'), f('b'), f('c'), f('z')]);
