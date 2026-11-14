// A getter on globalThis is called when the name is read as a global variable.
var reads = 0;
Object.defineProperty(globalThis, 'computedGlobal', { get: function () { reads++; return 'value ' + reads; }, configurable: true });
var a = computedGlobal;
function readInFunction() { return computedGlobal; }
var b = readInFunction();
var c = typeof computedGlobal;
var setLog = [];
Object.defineProperty(globalThis, 'settable', { set: function (v) { setLog.push(v); }, get: function () { return 'got'; }, configurable: true });
settable = 'assigned';
function assignInFunction() { settable = 'from function'; }
assignInFunction();
globalThis.__result = JSON.stringify([a, b, c, reads, settable, setLog]);
