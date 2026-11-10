// try/catch catches thrown values and runtime errors.
var out = [];
try { throw new Error('boom'); } catch (e) { out.push(e.name, e instanceof Error); }
try { null.x; } catch (e) { out.push(e.name); }
try { undefinedFunction(); } catch (e) { out.push(e.name); }
try { (1).toFixed(500); } catch (e) { out.push(e.name); }
try { new Array(-1); } catch (e) { out.push(e.name); }
try { JSON.parse('{'); } catch (e) { out.push(e.name); }
try { out.push('no throw'); } catch (e) { out.push('never'); }
globalThis.__result = JSON.stringify(out);
