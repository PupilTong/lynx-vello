// typeof on every kind of value.
var out = [typeof 1, typeof 'a', typeof true, typeof undefined, typeof null, typeof {}, typeof [], typeof function () {}, typeof class {}, typeof Symbol(), typeof (() => 1), typeof function* () {}, typeof new Number(1), typeof NaN];
globalThis.__result = JSON.stringify(out);
