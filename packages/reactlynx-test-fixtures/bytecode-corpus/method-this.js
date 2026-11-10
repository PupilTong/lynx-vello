// this in method calls, detached calls and computed member calls.
'use strict';
var o = { v: 'o', get: function () { return this === undefined ? 'undefined' : this.v; } };
var detached = o.get;
var arr = [o.get];
var out = [o.get(), o['get'](), detached(), (o.get)(), (0, o.get)(), arr[0]() === 'undefined' ? 'array' : 'other'];
globalThis.__result = JSON.stringify(out);
