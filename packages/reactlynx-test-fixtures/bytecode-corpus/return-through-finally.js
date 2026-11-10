// return inside try runs finally; finally can observe but not change the value unless it returns.
var log = [];
function a() { var x = 1; try { return x; } finally { x = 2; log.push('a finally'); } }
function b() { try { return 'try'; } finally { return 'finally'; } }
function c() { try { throw new Error(); } catch (e) { return 'catch'; } finally { log.push('c finally'); } }
function d() { try { throw 'lost'; } finally { return 'swallowed'; } }
function e() { for (var i = 0; i < 3; i++) { try { if (i === 1) return i; } finally { log.push('e' + i); } } }
globalThis.__result = JSON.stringify([a(), b(), c(), d(), e(), log]);
