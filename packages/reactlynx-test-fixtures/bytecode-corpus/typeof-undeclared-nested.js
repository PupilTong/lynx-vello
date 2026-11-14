// typeof on undeclared names inside nested, strict and arrow functions, and on names
// that a later script statement, an inner scope or a with object declares.
var log = [];
function a() {
  return function b() {
    'use strict';
    return () => function c() {
      return [typeof nowhere, typeof laterGlobal, typeof innerShadow, typeof nowhere === 'undefined'];
    };
  };
}
log.push(a()()()());
var laterGlobal = 1;
log.push(a()()()());
function shadowed() {
  var r = [typeof innerShadow];
  { let innerShadow = 'block'; r.push(typeof innerShadow); }
  r.push((function () { var innerShadow = 2; return typeof innerShadow; })());
  return r;
}
log.push(shadowed());
function viaWith(o) { with (o) { return [typeof fromWith, (function () { return typeof fromWith; })()]; } }
log.push(viaWith({ fromWith: 'x' }), viaWith({}));
globalThis.createdLater = true;
log.push(a()()()(), typeof createdLater);
globalThis.__result = JSON.stringify(log);
