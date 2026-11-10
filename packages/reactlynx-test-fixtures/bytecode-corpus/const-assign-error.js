// Assigning to a const binding throws a TypeError, also from a closure and with compound operators.
const c = 1;
var out = [];
try { c = 2; } catch (e) { out.push(e.name); }
try { c += 1; } catch (e) { out.push(e.name); }
try { c++; } catch (e) { out.push(e.name); }
function inner() { try { c = 3; } catch (e) { return e.name; } }
out.push(inner());
function local() { const d = 1; try { d = 2; } catch (e) { return e.name + ' ' + d; } }
out.push(local(), c);
globalThis.__result = JSON.stringify(out);
