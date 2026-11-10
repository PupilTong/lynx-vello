// Tagged templates evaluated in nested functions and loops: the strings array is
// one object per call site, substitutions capture variables from outer levels.
var sites = [];
function tag(strings, ...subs) {
  sites.push(strings);
  return strings.raw.join('|') + ' <' + subs.join(',') + '>';
}
function outer(a) {
  var b = a + 1;
  return function middle(c) {
    return function inner() {
      var out = [];
      for (var i = 0; i < 2; i++) out.push(tag`a=${a} b=${b} c=${c} i=${i}\t`);
      out.push((() => tag`arrow ${a + b + c}`)());
      out.push(tag`outer ${tag`nested ${c}`}`);
      return out;
    };
  };
}
var first = outer(1)(10)();
var second = outer(2)(20)();
var identity = [sites[0] === sites[1], sites[0] === sites[sites.length / 2], Object.isFrozen(sites[0])];
var method = { prefix: 'm', tag(s, v) { return this.prefix + s[0] + v; } };
globalThis.__result = JSON.stringify([first, second, identity, sites.length, method.tag`:${1}`]);
