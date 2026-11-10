// Compound assignment and ++/-- on a member of null or undefined throw TypeError.
var a = null, u, out = [];
var keyCalls = 0;
function key() { keyCalls++; return 'k'; }
try { a.b += 1; } catch (e) { out.push(e.name); }
try { a[key()] += 1; } catch (e) { out.push(e.name); }
try { a.b++; } catch (e) { out.push(e.name); }
try { u[key()]++; } catch (e) { out.push(e.name); }
try { --a.b; } catch (e) { out.push(e.name); }
try { a.b = 1; } catch (e) { out.push(e.name); }
try { u.x.y = 1; } catch (e) { out.push(e.name); }
try { a[0]; } catch (e) { out.push(e.name); }
out.push(keyCalls >= 0);
globalThis.__result = JSON.stringify(out);
