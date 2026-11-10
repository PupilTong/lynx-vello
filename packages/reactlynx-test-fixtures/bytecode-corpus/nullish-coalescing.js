// ?? returns the right side only for null and undefined, and short-circuits.
var log = [];
function side(v) { log.push(v); return v; }
var out = [null ?? 'a', undefined ?? 'b', 0 ?? 'c', '' ?? 'd', false ?? 'e', NaN ?? 'f', 'x' ?? side('never'), (null || undefined) ?? 'g', (0 ?? 1) || 2];
globalThis.__result = JSON.stringify([out.map(String), log]);
