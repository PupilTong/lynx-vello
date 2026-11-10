// In sloppy functions an undefined this is the global object and primitives are boxed.
function self() { return this; }
function kind() { return typeof this; }
globalThis.__result = JSON.stringify([self() === globalThis, kind.call(5), kind.call('s'), self.call(null) === globalThis, self.call(undefined) === globalThis]);
