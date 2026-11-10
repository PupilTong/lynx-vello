// Top-level let, const and class read and written by functions (at several nesting
// depths) before and after their declarations run.
var log = [];
function readLet() { return topLet; }
function writeLet() { topLet = 'written'; }
function readConst() { return function () { return topConst; }; }
function makeClass() { return function () { return function () { return new TopClass().id; }; }; }
function typeofLet() { return typeof topLet; }
function attempt(name, f) { try { log.push(name + ' ' + f()); } catch (e) { log.push(name + ' ' + e.name); } }
attempt('read let', readLet);
attempt('write let', writeLet);
attempt('read const', readConst());
attempt('new class', makeClass()());
attempt('typeof let', typeofLet);
let topLet = 'let value';
const topConst = 'const value';
class TopClass { constructor() { this.id = 'class ' + topLet; } }
attempt('read let', readLet);
attempt('write let', function () { writeLet(); return topLet; });
attempt('read const', readConst());
attempt('write const', function () { topConst = 'x'; });
attempt('new class', makeClass()());
attempt('typeof let', typeofLet);
attempt('global property', function () { return [globalThis.topLet, globalThis.TopClass]; });
globalThis.__result = JSON.stringify(log);
