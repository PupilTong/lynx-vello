// break or a throw in finally replaces a pending return or throw.
function a() { do { try { return 'from try'; } finally { break; } } while (false); return 'after loop'; }
function b() { try { return 'r'; } finally { throw new TypeError('t'); } }
function c() { for (;;) { try { throw new Error('x'); } finally { break; } } return 'swallowed by break'; }
var rb;
try { b(); } catch (e) { rb = e.name; }
globalThis.__result = JSON.stringify([a(), rb, c()]);
