// if / else if / else chains and truthiness of conditions.
function classify(v) {
  if (v === null) return 'null';
  else if (typeof v === 'number') { if (v > 0) return 'pos'; else if (v < 0) return 'neg'; else return 'zero'; }
  else if (v) return 'truthy';
  else return 'falsy';
}
globalThis.__result = JSON.stringify([null, 3, -3, 0, NaN, 'a', '', [], {}, undefined].map(classify));
