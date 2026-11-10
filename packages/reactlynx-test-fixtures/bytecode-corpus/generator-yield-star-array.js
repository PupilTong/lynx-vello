// yield* over arrays and strings.
function* g() { yield 0; var r = yield* [1, 2]; yield String(r); yield* 'ab'; }
globalThis.__result = JSON.stringify([...g()]);
