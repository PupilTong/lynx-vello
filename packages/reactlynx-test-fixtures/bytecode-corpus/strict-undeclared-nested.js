// Strict functions nested in sloppy code assign to undeclared names: plain,
// compound, update and destructuring assignments throw, assignments to names that
// exist as global properties succeed.
var log = [];
function attempt(name, f) { try { f(); log.push(name + ' ok'); } catch (e) { log.push(name + ' ' + e.name); } }
function outer() {
  'use strict';
  return function middle() {
    return function inner() {
      attempt('plain', function () { strictPlain = 1; });
      attempt('compound', function () { strictCompound += 1; });
      attempt('update', function () { strictUpdate++; });
      attempt('destructure', function () { [strictArray] = [1]; });
      attempt('object destructure', function () { ({ p: strictObject } = { p: 1 }); });
      attempt('existing', function () { createdBySloppy = 'strict write'; });
      attempt('property', function () { globalThis.viaProperty = 1; viaProperty = 2; });
    };
  };
}
createdBySloppy = 'sloppy';
outer()()();
function sloppyThenStrict() {
  sloppyCreated = 'sloppy creates';
  return (function () { 'use strict'; sloppyCreated += ' then strict appends'; return sloppyCreated; })();
}
log.push(sloppyThenStrict());
log.push(typeof strictPlain, typeof strictArray, createdBySloppy, viaProperty);
globalThis.__result = JSON.stringify(log);
