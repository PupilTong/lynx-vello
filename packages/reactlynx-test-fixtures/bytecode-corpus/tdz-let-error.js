// Reading a let binding before its declaration throws a ReferenceError.
var out = [];
try { out.push(early); } catch (e) { out.push(e.name); }
let early = 1;
out.push(early);
function f() {
  try { return inner; } catch (e) { out.push(e.name); }
  let inner = 2;
  return inner;
}
out.push(f());
{
  try { blockScoped = 3; } catch (e) { out.push('write ' + e.name); }
  let blockScoped;
}
try { let selfRef = selfRef + 1; } catch (e) { out.push('self ' + e.name); }
globalThis.__result = JSON.stringify(out);
