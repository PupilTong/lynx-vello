// Proxy traps for get, set, has, deleteProperty and apply, with Reflect forwarding.
var log = [];
var target = { a: 1 };
var p = new Proxy(target, {
  get(t, k, r) { log.push('get ' + String(k)); return k in t ? Reflect.get(t, k, r) : 'default'; },
  set(t, k, v) { log.push('set ' + k); return Reflect.set(t, k, v * 2); },
  has(t, k) { log.push('has ' + k); return k !== 'hidden'; },
  deleteProperty(t, k) { log.push('delete ' + k); return delete t[k]; },
});
p.b = 5;
var reads = [p.a, p.b, p.zzz, 'hidden' in p, 'x' in p, delete p.a];
var fp = new Proxy(function (x) { return x; }, { apply(t, self, args) { return 'wrapped ' + t(...args); } });
globalThis.__result = JSON.stringify([reads, fp(1), target, log]);
