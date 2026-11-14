// Generators cannot be constructed or resumed while running.
function* g() { yield 1; }
var out = [];
try { new g(); } catch (e) { out.push(e.name); }
var self;
function* reentrant() { try { self.next(); } catch (e) { yield e.name; } }
self = reentrant();
out.push(self.next().value);
function* thrower() { throw new RangeError('r'); }
try { thrower().next(); } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify(out);
