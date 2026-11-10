// Functions nested four and five levels deep read and write a declared global, a
// global declared later in the script and a global that is never declared.
var declared = 'declared';
function l1() {
  return function l2() {
    return function l3() {
      return function l4() {
        var before = [declared, typeof declaredLater, laterValue()];
        declared += ' + l4';
        declaredLater = 'set by l4';
        neverDeclared = 'created by l4';
        neverDeclared += '!';
        return function l5() {
          declaredLater += ' + l5';
          return before.concat([declared, declaredLater, neverDeclared, globalThis.neverDeclared]);
        };
      };
    };
  };
}
function laterValue() { return declaredLater; }
var deep = l1()()()();
var first = deep();
var declaredLater = 'initialiser runs after the first call';
var second = deep();
var missing;
try { (function () { return function () { return function () { return stillMissing; }; }; })()()(); } catch (e) { missing = e.name; }
globalThis.__result = JSON.stringify([first, second, declaredLater, neverDeclared, missing]);
