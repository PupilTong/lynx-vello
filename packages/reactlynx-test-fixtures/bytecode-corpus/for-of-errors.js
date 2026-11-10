// for-of over a non-iterable throws TypeError; next returning a non-object too.
var out = [];
try { for (const x of {}) {} } catch (e) { out.push(e.name); }
try { for (const x of 5) {} } catch (e) { out.push(e.name); }
try { for (const x of null) {} } catch (e) { out.push(e.name); }
var bad = { [Symbol.iterator]() { return { next() { return 1; } }; } };
try { for (const x of bad) {} } catch (e) { out.push(e.name); }
globalThis.__result = JSON.stringify(out);
