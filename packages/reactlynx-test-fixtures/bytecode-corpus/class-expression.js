// Class expressions, named and anonymous, and the inner name binding.
var Anon = class { m() { return 'anon'; } };
var Named = class Inner { who() { return Inner.name; } static self() { return Inner; } };
var made = (function (Base) { return class extends Base { m() { return 'mixed ' + super.m(); } }; })(Anon);
globalThis.__result = JSON.stringify([Anon.name, new Named().who(), Named.self() === Named, typeof Inner, new made().m()]);
