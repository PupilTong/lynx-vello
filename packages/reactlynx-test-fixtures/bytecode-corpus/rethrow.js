// A caught error can be rethrown and caught again with its identity.
var original = new RangeError('r');
var caught;
try {
  try { throw original; } catch (e) { e.seen = true; throw e; }
} catch (e2) { caught = e2; }
function wrap() { try { null.f(); } catch (e) { throw new Error('wrapped', { cause: e }); } }
var cause;
try { wrap(); } catch (e) { cause = e.cause.name; }
globalThis.__result = JSON.stringify([caught === original, caught.seen, caught.name, cause]);
