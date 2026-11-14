// await of a rejected promise throws inside the async function.
async function fail(name) { throw new TypeError(name); }
async function f() {
  var out = [];
  try { await fail('a'); } catch (e) { out.push('caught ' + e.name); }
  try { await Promise.reject(new RangeError('b')); } catch (e) { out.push('caught ' + e.name); }
  try { await null; out.push('no throw'); } catch (e) { out.push('never'); }
  return out;
}
f().then(function (v) { globalThis.__result = JSON.stringify(v); });
