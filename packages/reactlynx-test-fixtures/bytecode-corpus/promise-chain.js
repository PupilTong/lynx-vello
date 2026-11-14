// Promise then/catch/finally chains and thenable adoption.
var log = [];
var thenable = { then: function (res) { res('from thenable'); } };
Promise.resolve(1)
  .then(function (v) { log.push(v); return v + 1; })
  .then(function (v) { log.push(v); throw new Error('e'); })
  .then(function () { log.push('skipped'); })
  .catch(function (e) { log.push('catch ' + e.name); return thenable; })
  .finally(function () { log.push('finally'); return 'ignored'; })
  .then(function (v) { log.push(v); return Promise.all([1, Promise.resolve(2)]); })
  .then(function (v) { log.push(v); return Promise.allSettled([Promise.reject(3), 4]); })
  .then(function (v) { log.push(v.map(function (s) { return s.status; })); globalThis.__result = JSON.stringify(log); });
