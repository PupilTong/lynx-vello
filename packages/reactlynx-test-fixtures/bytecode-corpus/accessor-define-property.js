// Object.defineProperty accessors and data descriptors.
var o = {};
var store = 0;
Object.defineProperty(o, 'x', { get: function () { return store; }, set: function (v) { store = v + 1; }, enumerable: false, configurable: true });
Object.defineProperty(o, 'ro', { value: 9, writable: false });
o.x = 10;
o.ro = 1;
var strictWrite = (function () { 'use strict'; try { o.ro = 2; return 'ok'; } catch (e) { return e.name; } })();
var d = Object.getOwnPropertyDescriptor(o, 'ro');
globalThis.__result = JSON.stringify([o.x, store, o.ro, strictWrite, Object.keys(o), d.writable, d.enumerable, d.configurable]);
