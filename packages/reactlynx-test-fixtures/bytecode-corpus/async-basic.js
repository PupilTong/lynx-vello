// An async function returns a promise; await resumes with the settled value.
var log = [];
async function f(x) { log.push('start ' + x); var y = await x; log.push('resumed ' + y); return y * 2; }
var p = f(21);
log.push('sync after call');
p.then(function (v) { log.push('result ' + v); globalThis.__result = JSON.stringify([p instanceof Promise, log]); });
