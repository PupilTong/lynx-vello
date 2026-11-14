// throw() on a suspended generator resumes it with an exception.
function* g() {
  while (true) {
    try { yield 'waiting'; } catch (e) { yield 'caught ' + e; }
  }
}
var it = g();
var out = [it.next().value, it.throw('a').value, it.next().value, it.throw('b').value];
function* noCatch() { yield 1; }
var n = noCatch();
n.next();
try { n.throw(new TypeError('x')); } catch (e) { out.push(e.name); }
out.push(JSON.stringify(n.next()));
var unstarted = noCatch();
try { unstarted.throw('pre'); } catch (e) { out.push('pre ' + e); }
globalThis.__result = JSON.stringify(out);
