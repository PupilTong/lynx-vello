// Top-level function declarations redefined by later declarations, by assignment
// and by a var with an initialiser; callers bound before and after each change.
var log = [];
log.push(which());
var early = which;
function which() { return 'first declaration'; }
log.push(which());
function which() { return 'second declaration'; }
log.push(which(), early === which);
which = function () { return 'assigned expression'; };
log.push(which(), early());
var which = function () { return 'var initialiser'; };
log.push(which());
function caller() { return which(); }
log.push(caller());
which = 'not a function';
try { caller(); } catch (e) { log.push(e.name); }
function selfReplace() { selfReplace = function () { return 'replaced'; }; return 'original'; }
log.push(selfReplace(), selfReplace(), typeof globalThis.selfReplace);
globalThis.__result = JSON.stringify(log);
