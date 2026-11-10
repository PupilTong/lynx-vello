// && and || return an operand and skip the right side when decided.
var log = [];
function t(v) { log.push(v); return v; }
var r1 = t(0) && t(1);
var r2 = t(1) && t(2);
var r3 = t('') || t('x');
var r4 = t('y') || t('z');
var r5 = !t(null) && t(3);
globalThis.__result = JSON.stringify([r1, r2, r3, r4, r5, log]);
