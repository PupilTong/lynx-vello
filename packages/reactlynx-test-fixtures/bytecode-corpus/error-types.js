// Native error constructors, subclassing and properties.
var types = [Error, TypeError, RangeError, SyntaxError, ReferenceError, EvalError, URIError];
var out = types.map(function (T) { var e = new T('m'); return [e.name, e.message, e instanceof Error, Object.prototype.toString.call(e)]; });
class MyError extends TypeError { constructor(m) { super(m); this.name = 'MyError'; } }
var me = new MyError('x');
out.push([me.name, me instanceof TypeError, me instanceof MyError, String(me)]);
try { decodeURIComponent('%'); } catch (e) { out.push(e.name); }
out.push(String(new Error('msg')), Error('no new') instanceof Error);
globalThis.__result = JSON.stringify(out);
