// A global let shadows a globalThis property of the same name for identifier reads.
globalThis.shadowed = 'property';
let shadowed = 'lexical';
function read() { return shadowed; }
globalThis.__result = JSON.stringify([shadowed, globalThis.shadowed, read()]);
