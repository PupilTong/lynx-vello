// An inner function assigns to seven captured var bindings and three captured let
// bindings, using closure-variable indices past the short forms.
function outer() {
  var c0 = 0, c1 = 1, c2 = 2, c3 = 3, c4 = 4, c5 = 5, c6 = 6;
  let l0 = 'l0', l1 = 'l1', l2 = 'l2';
  function bump() {
    c0 = c0 + 1;
    c1 = 'one';
    c2 = 'two';
    c3 = 'three';
    c4 = 'four';
    c5 = c6 = 'six';
    l0 = l1 = 'lexical';
    l2 += '!';
    var chained = [c4 = 'four again', c3 = 'three again', c2 = 'two again', c1 = 'one again', c6 = 'six again'];
    c5++;
    c6--;
    return chained;
  }
  var r = bump();
  return [r, c0, c1, c2, c3, c4, c5, c6, l0, l1, l2];
}
globalThis.__result = JSON.stringify(outer());
