// Static fields, initialised once in order, with this referring to the class.
// The initialisers make no calls: tasm 0.0.53 crashes compiling a call in a field initialiser.
var log = [];
class S {
  static a = (log[log.length] = 'a', 1);
  static b = S.a + 1;
  static self = this;
  static ['comp' + 'uted'] = 'c';
}
class T extends S { static c = this.b * 10; }
globalThis.__result = JSON.stringify([S.a, S.b, S.self === S, S.computed, T.c, T.hasOwnProperty('a'), log]);
