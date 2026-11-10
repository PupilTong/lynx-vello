// Arrow functions take this from the enclosing function.
var o = {
  name: 'o',
  regular: function () { return [1].map(function () { return this; })[0] === undefined || true; },
  arrow: function () { return [1].map(() => this.name)[0]; },
  nested: function () { var self = this; return (() => () => this === self)()(); },
};
function Outer() { this.v = 1; this.get = () => this.v; }
var x = new Outer();
var detached = x.get;
globalThis.__result = JSON.stringify([o.arrow(), o.nested(), detached(), o.regular()]);
