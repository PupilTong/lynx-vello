// Any value can be thrown.
var out = [];
var values = [1, 'str', null, undefined, { k: 1 }, [1], true, Symbol.iterator];
for (var i = 0; i < values.length; i++) {
  try { throw values[i]; } catch (e) { out.push(e === values[i]); }
}
function thrower() { throw 'from function'; }
try { thrower(); } catch (e) { out.push(e); }
globalThis.__result = JSON.stringify(out);
