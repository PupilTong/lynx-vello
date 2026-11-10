// finally runs after try and after catch, in order.
var log = [];
try { log.push('try'); } finally { log.push('finally'); }
try { log.push('try2'); throw 1; } catch (e) { log.push('catch ' + e); } finally { log.push('finally2'); }
try {
  try { throw 'inner'; } finally { log.push('inner finally'); }
} catch (e) { log.push('outer catch ' + e); }
globalThis.__result = JSON.stringify(log);
