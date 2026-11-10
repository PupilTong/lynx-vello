// A throw in an async function rejects its promise instead of throwing synchronously.
var log = [];
async function f() { log.push('body'); throw new SyntaxError('s'); }
var p;
try { p = f(); log.push('no sync throw'); } catch (e) { log.push('sync throw'); }
p.catch(function (e) { log.push('rejected ' + e.name); }).then(function () { globalThis.__result = JSON.stringify(log); });
