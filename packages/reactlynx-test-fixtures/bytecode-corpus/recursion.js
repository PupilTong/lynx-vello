// Direct and mutual recursion.
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
function isEven(n) { return n === 0 ? true : isOdd(n - 1); }
function isOdd(n) { return n === 0 ? false : isEven(n - 1); }
function depth(n) { return n === 0 ? 0 : 1 + depth(n - 1); }
var ackermann = function ack(m, n) { return m === 0 ? n + 1 : n === 0 ? ack(m - 1, 1) : ack(m - 1, ack(m, n - 1)); };
globalThis.__result = JSON.stringify([fib(15), isEven(10), isOdd(7), depth(500), ackermann(2, 3)]);
