// void evaluates its operand and yields undefined.
var count = 0;
var r = void count++;
var s = void 0 === undefined;
globalThis.__result = JSON.stringify([typeof r, count, s, typeof void 'x']);
