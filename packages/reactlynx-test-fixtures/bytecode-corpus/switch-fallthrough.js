// switch with fallthrough, break and grouped cases.
function f(x) {
  var log = [];
  switch (x) {
    case 1: log.push('one');
    case 2: log.push('two'); break;
    case 3:
    case 4: log.push('three-four');
    case 5: log.push('five'); break;
    default: log.push('default');
  }
  return log.join(',');
}
globalThis.__result = JSON.stringify([1, 2, 3, 4, 5, 6].map(f));
