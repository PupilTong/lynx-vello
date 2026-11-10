// catch without a binding.
var log = [];
try { throw new Error(); } catch { log.push('caught'); }
try { JSON.parse('x'); } catch { log.push('json'); } finally { log.push('finally'); }
function f() { try { return JSON.parse('{}'); } catch { return 'never'; } }
log.push(typeof f());
globalThis.__result = JSON.stringify(log);
