// typeof on a binding in its temporal dead zone throws.
var r;
try { r = typeof later; } catch (e) { r = e.name; }
let later = 1;
var r2;
{
  try { r2 = typeof inBlock; } catch (e) { r2 = e.name; }
  const inBlock = 2;
}
globalThis.__result = JSON.stringify([r, r2]);
