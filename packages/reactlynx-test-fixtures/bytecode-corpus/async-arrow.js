// Async arrow functions capture this and arguments lexically.
function Holder() {
  this.v = 'holder';
  this.get = async () => (await this.v) + arguments.length;
}
var h = new Holder(1, 2);
var double = async x => x * 2;
var noParens = async () => 'np';
Promise.all([h.get(), double(4), noParens()]).then(function (v) { globalThis.__result = JSON.stringify(v); });
