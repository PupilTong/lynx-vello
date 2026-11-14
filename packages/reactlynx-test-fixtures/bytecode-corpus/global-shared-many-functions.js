// One global var read, assigned, updated and compound-assigned from many separate
// functions, arrows, methods, getters and nested closures.
var counter = 0;
function inc() { counter++; }
function dec() { --counter; }
function add(n) { counter += n; }
function mul(n) { counter = counter * n; }
function read() { return counter; }
var arrowInc = () => counter++;
var obj = {
  bump() { counter += 10; return counter; },
  get value() { return counter; },
  set value(v) { counter = v; },
};
function makeAdder() { return function (n) { return function () { counter += n; return counter; }; }; }
function* gen() { while (counter < 100) yield counter += 25; }
class Holder { static take() { var c = counter; counter = 0; return c; } snapshot() { return counter; } }
var log = [];
inc(); inc(); dec(); log.push(read());
add(5); mul(3); log.push(read());
log.push(arrowInc(), counter);
log.push(obj.bump(), obj.value);
obj.value = 40; log.push(counter);
log.push(makeAdder()(2)(), makeAdder()(3)());
log.push([...gen()]);
log.push(new Holder().snapshot(), Holder.take(), counter);
globalThis.__result = JSON.stringify(log);
