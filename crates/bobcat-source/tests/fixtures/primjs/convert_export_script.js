// A named Lepus chunk converted with Completion::ExportScript: each call
// runs the whole script; its top-level declarations are globals, `this` is
// what the caller passes, imports are live, and the call returns the
// completion value.
var calls = (typeof calls === 'number' ? calls : 0) + 1;
function chunkHelper() { return 'helper:' + calls; }
var sawGlobalThis = this === globalThis;
calls + ':' + counter + ':' + sawGlobalThis;
