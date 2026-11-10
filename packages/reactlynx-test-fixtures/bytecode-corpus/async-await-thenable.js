// await on a thenable calls its then; await on a plain value continues later.
var log = [];
var thenable = { then(resolve) { log.push('then called'); resolve('thenable value'); } };
async function f() { var a = await thenable; var b = await 5; return [a, b]; }
f().then(function (v) { log.push(v); globalThis.__result = JSON.stringify(log); });
log.push('sync');
