// delete on global bindings: var and function declarations stay, implicit globals go.
var declared = 1;
implicitOne = 2;
globalThis.explicitProp = 3;
function declaredFn() {}
var r1 = delete globalThis.declared;
var r2 = delete implicitOne;
var r3 = delete globalThis.explicitProp;
var r4 = delete globalThis.declaredFn;
var r5 = delete neverExisted;
globalThis.__result = JSON.stringify([r1, r2, r3, r4, r5, typeof declared, typeof implicitOne, typeof explicitProp, typeof declaredFn]);
