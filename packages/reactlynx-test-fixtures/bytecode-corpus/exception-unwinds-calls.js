// An exception unwinds several call frames and loop bodies.
var log = [];
function level3() { log.push('3'); throw new Error('deep'); }
function level2() { try { level3(); } finally { log.push('2 finally'); } }
function level1() { for (var i = 0; i < 3; i++) { log.push('loop ' + i); level2(); } }
try { level1(); } catch (e) { log.push('caught ' + e.message); }
var it = [1, 2, 3].map(function (x) { try { if (x === 2) throw x; return x; } catch (v) { return -v; } });
globalThis.__result = JSON.stringify([log, it]);
