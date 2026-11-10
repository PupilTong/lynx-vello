// yield* delegates to another generator and receives its return value.
function* inner() { var x = yield 'i1'; yield 'got ' + x; return 'inner done'; }
function* outer() { var r = yield* inner(); yield r; }
var it = outer();
var out = [it.next().value, it.next('sent').value, it.next().value, it.next().done];
function* tree(n) { if (n > 0) { yield* tree(n - 1); yield n; yield* tree(n - 1); } }
globalThis.__result = JSON.stringify([out, [...tree(3)]]);
