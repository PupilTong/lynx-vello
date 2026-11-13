// Free names of a converted script: imports (live bindings, read-only),
// a name bound to undefined, a declared name that is also in the import
// list (the script's global wins), and the completion value.
var log = [];
log.push(typeof lynx, lynx.tag);
function readCount() { return counter; }
log.push(readCount());
bump();
log.push(readCount(), counter);
try { lynx = 1; } catch (e) { log.push(e.name); }
(function () { 'use strict'; try { counter = 2; } catch (e) { log.push(e.name); } })();
var shadowed = 'script';
log.push(shadowed, typeof window);
try { window = 3; } catch (e) { log.push(e.name); }
globalThis.__log = log;
'completion:' + log.length;
